# Caddy resilience plan

## Status (2026-05-01)

- **Fix A (rollback on rejected replay) — done.** Added
  `CaddyStartupError::ConfigRejected`. The blue/green upgrade arm of
  `ensure_caddy_running` now stops the new slot and returns the error
  instead of committing the slot swap when the replay POST fails. The
  fresh-start arm intentionally keeps warn-and-continue (no previous slot
  to fall back to).
- **Fix B (cache `ProxyConfig`, not raw Caddy JSON) — done.** Schema v52
  wipes existing rows; cache helpers now persist the Seedling-internal
  `ProxyConfig` and the replay path calls `build_caddy_config` at apply
  time, eliminating format drift between the cached value and the running
  Caddy version. The redundant `caddy_json` field on `ProxyBuildResult`
  was dropped.
- **Fix E (start_transient during systemd restart window) — addressed by
  unrelated work.** `start_slot` already handles a lingering unit by
  resetting/stopping it and waiting for it to unload before starting a
  new transient. Different mechanism than the plan proposed but the
  spurious-error symptom is gone.
- **Fix C (immediate tick after replay failure)** — no code change made;
  the gap was already minimal and is now narrower with A+B.
- **Fix D (per-ingress fault on repeated apply failures)** — still
  outstanding. Today only a system-level `proxy_failed` fault is filed.
  Per-ingress faults remain a Phase 7 concern.
- **Gap 1 (`is_healthy` only checks admin liveness) — done.**
  `is_healthy_impl` now parses the `GET /config/` response and returns
  `false` for a JSON `null` body, so the "admin alive but no config
  loaded" state is correctly treated as unhealthy by both the observer
  (`observe.ingress`) and the running-with-correct-image branch in
  `ensure_caddy_running`.

## What the current code does

`ensure_caddy_running` is called on every reconciler tick (10-second timeout) and at
startup. It is intended to be idempotent: return immediately when Caddy is already up,
start/restart it otherwise. The blue/green upgrade path is also wired through here.

`apply_config` / `apply_raw_json` both POST to Caddy's `/config/` and check the HTTP
response status. So a synchronous rejection from Caddy (malformed JSON, unknown field,
bad value) is caught and propagated as `CaddyError::Api { status, body }`.

The proxy config cache (`caddy_proxy_config` table) is written only after a successful
`apply_config`, so whatever is cached was accepted by Caddy at the time it was stored.

## Known gaps

### 1. `is_healthy` only checks the admin API, not config validity

`GET /config/` returns `null` with a 200 on a fresh Caddy instance that has no config
at all. The health gate in `poll_until_healthy` is therefore "the admin process is alive",
not "a valid routing config is loaded". A new Caddy version with a breaking config format
can pass this check and be considered healthy while routing nothing.

### 2. Cache replay failure is silently swallowed

Both the fresh-start and upgrade paths do:

```rust
if let Ok(Some(json)) = read_cached_proxy_json(data_dir)
    && let Err(e) = CaddyProxy::new(addr).apply_raw_json(&json).await
{
    tracing::warn!("failed to apply cached proxy config...: {e}");
}
```

The failure is a `warn!` and execution continues. `ensure_caddy_running` returns `Ok(addr)`
regardless. The caller has no way to know that Caddy is running with an empty config.
The reconciler will push a fresh config on the next tick, so the gap is bounded — but it is
silent, and in the breaking-change case (see below) the fresh push also fails.

### 3. Cached config bakes in live container IPs

The cache stores the fully-built raw Caddy JSON, including the upstream IPv6 addresses of
containers at the time the config was last applied. If a pod container is restarted and gets
a new SLAAC address before Caddy is restarted, the replayed config points to the stale
address. Caddy accepts it (syntactically valid), proxy silently misroutes. The reconciler
corrects this within one tick.

### 4. Breaking API change causes a permanent silent outage with no rollback

This is the most dangerous case. The sequence:

1. Reconciler tick detects image ID mismatch, starts new slot.
2. `poll_until_healthy` polls `GET /config/` — new Caddy starts fine, 200 returned.
3. `apply_raw_json(cached_config)` — fails if format changed — swallowed as `warn!`.
4. `write_active_container` commits the new slot to the DB.
5. Old slot is stopped (best-effort, errors ignored).
6. `ensure_caddy_running` returns `Ok(new_addr)` — upgrade considered done.
7. Every subsequent reconciler tick: `proxy::apply` posts the current config (built by
   `build_caddy_config`) to the new Caddy — also fails if the API format changed — logged
   as `error!`, proxy phase skipped.
8. Proxy is down permanently. No rollback mechanism exists.

The upgrade is irreversible once `write_active_container` runs: the old slot has been
stopped (or at least told to stop), and there is no code path to un-commit the slot swap
from within the running system.

### 5. `start_transient` during a systemd restart window

When Caddy crashes and systemd is restarting it (the container has been removed by `--rm`
but the new one is not yet up), `ensure_caddy_running` sees `inspect` returning `None`,
treats it as "no container", and calls `start_slot` → `start_transient`. systemd likely
rejects this because it already owns the unit, returning a `Process` error. The tick then
skips the proxy phases. This is harmless — the next tick will find Caddy healthy — but it
means a crash-and-restart cycle produces one or more spurious `error!` log lines that look
like a real failure.

## Proposed fixes

### A. Roll back the upgrade if config replay fails

In the upgrade arm of `ensure_caddy_running`, treat a `apply_raw_json` rejection as a
signal that the new image is incompatible. Stop the new slot, leave the old one running,
and return an error instead of committing the slot swap. Sketch:

```rust
if let Err(e) = CaddyProxy::new(new_addr).apply_raw_json(&json).await {
    warn!("upgraded Caddy rejected cached config ({e}); rolling back");
    stop_slot(other, process, container).await;
    return Err(CaddyStartupError::ConfigRejected { source: e });
}
```

This requires defining `CaddyStartupError::ConfigRejected` and deciding what happens when
there is no cached config (no rollback needed — an empty Caddy is fine, just apply on the
next tick).

Open questions:
- Should we attempt the fresh-config push (from reconciler state rather than cache) before
  deciding to roll back? That would avoid a rollback when only the cached format is stale but
  the new Caddy is otherwise fine.
- If there is no cached config, the upgrade should still proceed; the reconciler will push
  the live config shortly after.

### B. Cache the Seedling ProxyConfig, not the raw Caddy JSON

The cache currently stores `caddy_json` (the output of `build_caddy_config`). If instead we
cache the Seedling `ProxyConfig` and call `build_caddy_config` at replay time, the replay
will produce config in whatever format the current code knows how to generate — which should
be compatible with whatever Caddy version is currently running. This removes the format-drift
risk between the cached value and the new Caddy version.

The trade-off: if `build_caddy_config` itself has a bug introduced alongside a Caddy upgrade,
the fresh build is also broken. But that is the same situation as the reconciler's per-tick
push, so it does not make things worse.

### C. Trigger an immediate tick after cache replay

When cache replay fails, the reconciler does not know about it. Since `ensure_caddy_running`
runs at the start of the global reconciler tick, the proxy phase follows immediately within
the same tick — so the gap is already minimal.

### D. Treat repeated config-apply failures as a fault

Currently `proxy::apply` logs `error!` and returns on failure, with no per-ingress fault
filed. If `apply_config` fails on N consecutive ticks for a given app's ingress resources,
a fault should be filed against those resources so the degraded state is visible via the OI
(`ListFaults`, `GetStatus`, `DescribeApp`). This is a Phase 7 concern but worth noting here
as the mechanism that would surface a breaking-change outage to operators.

### E. Distinguish "unit already being restarted by systemd" from "unit missing"

When `inspect` returns `None` during a systemd restart window, we currently fall through to
`start_slot` which fails. A lighter fix is to call `process.unit_state` before attempting
`start_slot`: if the unit exists in any state (activating, deactivating, failed) we know
systemd is managing it and we should just poll rather than trying to start a new instance.
This suppresses the spurious `Process` error and turns the restart window into a quiet wait.

## Things still to investigate

- Does `podman run --name <same> ...` consistently produce the same SLAAC address across
  restarts? If the MAC is stable for a given container name, the admin address never drifts
  and gap 3 is less of a concern. If not, there is a window where `caddy_admin_addr` is
  stale even though `ensure_caddy_running` returned `Ok`.
- What does systemd do when `start_transient` is called for an already-managed unit?
  Understanding the exact error makes it possible to detect the restart-window case cleanly.
- Is there a Caddy API endpoint more suitable for a liveness check than `GET /config/`?
  Something that returns non-200 if no config is loaded would let `is_healthy` distinguish
  "admin API up" from "routing config loaded". Caddy's `/healthz` endpoint (if present) may
  be more appropriate, or checking that the config response is non-null.
- What is the right behaviour when there is no cached config and the upgrade path succeeds?
  Currently we proceed silently; the reconciler fixes it. This seems correct but should be
  confirmed as intentional rather than accidental.