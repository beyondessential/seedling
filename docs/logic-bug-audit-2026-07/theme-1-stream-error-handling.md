# Theme 1: Error responses on bidi streams are discarded by clients

> Companion to the [logic bug audit](../logic-bug-audit-2026-07.md), cross-cutting theme 1.

## The failure pattern

Subscription-style OI requests (`/events/subscribe`, `/logs/stream`) share a three-step handshake, implemented server-side in `crates/core/src/oi/server.rs` (`handle_bidi_stream`):

1. Client opens a bidi stream, writes the request JSON, half-closes.
2. Server writes exactly one envelope on the bidi — `{"result":{}}` or `{"error":{...}}` — and finishes it. Per `i[stream.control]`, the stream FIN is the message boundary; the envelope is **not** newline-terminated.
3. Only on success does the server open the server-initiated uni stream that carries the actual data (`i[stream.events]`, `i[stream.logs]`).

The error branches are real and routine: `server_busy` from the stream-concurrency semaphore (`server.rs:286-306`), `requirements_invalid` / `not_found` from `/logs/stream` param validation, `server_busy` from a journal-open failure (`server.rs:383-417`). In every error branch the server finishes the bidi and returns — **no uni stream is ever opened, and the connection stays alive** (keep-alive PINGs every 10 s, `server.rs:147`, defeat the 30 s idle timeout).

Each consumer re-implements the client side of this handshake by hand, and each re-implementation gets a different subset wrong:

- `OiClient::subscribe_events` (`crates/protocol/src/client.rs:342-370`) does `BufReader::read_line` on the bidi, discards the line unread, and calls `accept_uni()`. On an error envelope it parks forever. It also uses the wrong framing primitive: `read_line` only returns here because the FIN produces EOF — the envelope carries no `\n`.
- `DaemonConn::start_log_stream` (`crates/web/src/daemon.rs:233-266`) is a byte-for-byte copy of the same mistake on a dedicated connection: `read_line`, ignore, `accept_uni()`, hang, leaking the connection and the browser's WT stream.
- `run_subscribe_session` (`crates/ctl/src/subscribe.rs:88-98`) gets the mechanics right — `read_to_end`, check for `"error"` before `accept_uni` — but then misclassifies the outcome as `SessionOutcome::GracefulClose`, so `ctl events` exits 0 on a rejected subscription.
- `run_log_session` (`crates/ctl/src/logs.rs:50-72`) is the one fully correct implementation: `read_to_end`, parse, return `[code] message` as an error, only then `accept_uni`.

Four hand-rolled copies of a handshake with three distinct failure modes (error envelope, empty FIN, uni-never-opens) is why this keeps going wrong: the happy path works on first test, and the error path is invisible until a transient `server_busy` bricks a long-lived consumer. None of the copies bounds the `accept_uni` wait either, so even a confirmed-OK handshake can hang if the server's `open_uni` fails (its failure path at `server.rs:355-361` only logs and returns).

## Affected findings

| Finding | Section | Severity |
|---|---|---|
| `subscribe_events` swallows error responses and then blocks forever waiting for a uni stream that will never arrive (H16) | [§1](../logic-bug-audit-2026-07.md#1-protocol-crate-cratesprotocol) | high |
| `/logs/stream` hangs forever when the daemon returns an error response (H17) | [§17](../logic-bug-audit-2026-07.md#17-web-crate) | high |
| `events` exits 0 when the server rejects the subscription | [§16](../logic-bug-audit-2026-07.md#16-daemon-and-ctl-crates) | low |

Downstream of H16: the web event broker's reconnect loop (`crates/web/src/event_broker.rs:81-116`) is built entirely on `subscribe_events` returning promptly, so its back-off machinery is dead weight until the helper is fixed.

## Would a high-level change help?

**Yes.** The three broken call sites and the one correct one are the same function with different bugs. All four:

- send `{ "method": ..., "actor": ..., "params": ... }` on a fresh bidi and finish it;
- must read the single response envelope to FIN and classify it;
- must accept exactly one server-initiated uni stream only after a confirmed OK.

The only per-caller variation is what happens *after* the uni stream is obtained (print, broker-publish, WT-relay) and how errors map to caller policy (retry vs exit code vs HTTP-ish error line). Those stay with the callers; the handshake does not. `OiClient::request` (`client.rs:396-442`) already contains the correct envelope classification (untagged `Ok { result } / Err { error }` → `ClientError::Api`); the fix is to reuse it rather than let each caller re-derive it. This is exactly the shape where one shared helper eliminates the class: the bug is not subtle logic, it is per-caller re-implementation of a wire contract.

## Proposed pattern

Add a typed subscription-open helper to `crates/protocol/src/client.rs`, next to `request`, and delete every hand-rolled handshake. Contract:

- **Consumes the full bidi response** with `read_to_end` (64 KiB bound), matching `i[stream.control]`'s FIN-is-the-boundary framing. Never `read_line`.
- **Classifies every outcome**: `{"result":...}` → proceed; `{"error":{code,message}}` → `Err(ClientError::Api)`; FIN with zero bytes → `Err(ClientError::Protocol)`; unparseable or oversize → `Err(ClientError::Protocol)`.
- **`accept_uni` is unreachable without a confirmed OK**, and is wrapped in a handshake timeout so the server's log-and-return `open_uni` failure path cannot hang the client.

```rust
const SUBSCRIBE_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

impl OiClient {
    /// Shared by `request()` and `open_subscription()`.
    fn parse_response(bytes: &[u8]) -> Result<Value, ClientError> {
        if bytes.is_empty() {
            return Err(ClientError::Protocol(
                "server closed the stream without a response".into(),
            ));
        }
        // ... the existing untagged Ok/Err envelope match from request() ...
    }

    /// Send a subscription-style request; return the server-initiated uni
    /// stream carrying the data, or a typed error. Never hangs on an error
    /// envelope, an empty stream, or a server that fails to open the uni.
    pub async fn open_subscription(
        &self,
        method: &str,
        params: Value,
    ) -> Result<RecvStream, ClientError> {
        let (mut send, mut recv) = self.conn.open_bi().await.map_err(transport)?;
        let req = serde_json::to_vec(&json!({
            "method": method, "actor": &self.actor, "params": params,
        }))
        .expect("serialise never fails");
        send.write_all(&req).await.map_err(transport)?;
        send.finish().map_err(transport)?;

        let body = recv.read_to_end(64 * 1024).await.map_err(transport)?;
        Self::parse_response(&body)?;

        tokio::time::timeout(SUBSCRIBE_HANDSHAKE_TIMEOUT, self.conn.accept_uni())
            .await
            .map_err(|_| ClientError::Protocol(
                "server accepted the subscription but never opened the data stream".into(),
            ))?
            .map_err(transport)
    }
}
```

`subscribe_events` becomes `self.open_subscription("/events/subscribe", json!({})).await`. Migrating callers: the web event broker (via `subscribe_events`), `DaemonConn::start_log_stream` (which keeps returning `(OiClient, RecvStream)` so the dedicated connection stays alive, but takes `method`/`params` instead of pre-serialised `request_bytes`), `crates/ctl/src/logs.rs`, and `crates/ctl/src/subscribe.rs`. The ctl subscribe path additionally maps `ClientError::Api` to a new failing `SessionOutcome` so the process exits non-zero, fixing the §16 finding as a by-product; transport errors keep feeding the existing reconnect loop.

## What it prevents — and what it does not

Prevents, structurally rather than per-caller:

- infinite `accept_uni` hangs on `server_busy`, `requirements_invalid`, `not_found`, and journal failures (H16, H17);
- silent discarding of the server's error message — callers now receive `code` and `message` typed;
- the empty-FIN hang (server drops the request line and finishes, `server.rs:317-319`);
- hangs when the server's `open_uni` fails post-OK, via the handshake timeout;
- the wrong-framing trap (`read_line` against a non-newline-terminated envelope) recurring in the next consumer.

Does not prevent:

- callers misclassifying the returned error — `ctl events`' exit-0 bug is a policy bug at the call site and needs its own one-line fix plus test;
- the shell handshake going wrong: `/shells/start` uses genuinely different framing (newline-delimited JSON on a bidi that stays open, uni stream IDs announced in the handshake, `i[stream.shell.framing]`) and is out of scope for this helper;
- data-phase defects on the uni stream itself (the web broker's lag/duplication findings in §17 are separate);
- a misbehaving server that opens the uni before writing the envelope, or writes a non-envelope — though both now surface as prompt `Protocol` errors instead of hangs.

## Migration path

1. Extract `parse_response` from `OiClient::request`; add `open_subscription` with the timeout; reimplement `subscribe_events` on top of it. Fixes H16 and, transitively, the web event broker. (~half a day, including the stub-server tests below.)
2. Rework `DaemonConn::start_log_stream` onto `open_subscription`; surface `ClientError::Api` to `crates/web/src/wt.rs:240-261` so the browser gets the daemon's error line instead of a spinner. Fixes H17. (~half a day with the caller change.)
3. Port `crates/ctl/src/logs.rs` and `crates/ctl/src/subscribe.rs` to the helper, deleting their hand-rolled handshakes; add the failing `SessionOutcome` mapping for `Api` errors in `subscribe.rs` (exit 1). (~2 hours.)
4. Add the spec item, tracey annotations, and the CI allowlist check described below. (~2 hours.)

Each step is independently shippable; step 1 alone removes the worst hang.

## Enforcement

- **Tests**: `TestOi` (`crates/core/src/oi/test_support.rs`) drives `dispatch` directly and never touches `handle_bidi_stream`, so it cannot exercise this handshake. Add a small quinn loopback stub server to the protocol crate's tests (or boot the real `oi::run`, which returns its endpoints) and cover the four outcomes: error envelope → prompt `ClientError::Api`; FIN with no bytes → `Protocol`; OK but no uni within the timeout → `Protocol`; OK plus uni → stream returned. Wrap each assertion in a short `tokio::time::timeout` so a regression fails fast instead of hanging CI.
- **Spec item** in `docs/spec/interface.md`, under Streams, phrased as a requirement on the wire contract rather than the code: *for subscription-style requests, an error response on the bidirectional stream terminates the request — the server does not open the unidirectional stream and clients must not wait for one; clients must treat closure of the bidirectional stream without a response envelope as an error and surface the envelope's error to their caller.* Annotate `open_subscription` and the stub-server tests with the corresponding `i[impl ...]` / `i[verify ...]` references so `tracey query status` tracks coverage.
- **CI grep**: `accept_uni` outside an allowlist (`crates/protocol/src/client.rs`, the shell paths `crates/ctl/src/shell.rs` and the web `UniRouter` dispatcher in `crates/web/src/daemon.rs`) fails the build with a pointer at `open_subscription`. Cheap, and it catches the fifth hand-rolled copy before review does.
- **Review checklist**: any new consumer of a server-initiated uni stream must either use `open_subscription` or justify why its framing differs (as the shell protocol does); `read_line` on an OI bidi response is always wrong — the envelope is FIN-delimited.
