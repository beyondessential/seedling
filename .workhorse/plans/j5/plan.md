# Issue certificates through Canopy

Implements the specs for the default chain (Canopy, then HTTP-01), the fall-through ladder, the Canopy strategy and the Canopy-only policy: `tls.strategy.*`, `tls.chain.*`, `tls.canopy.*` in `docs/spec/runtime.md`, `tls.policy.set-canopy` and the `chain` hostname field in `docs/spec/interface.md`, and the policies and hostnames sections of `docs/spec/web.md`.

## What the code does today

These findings come from reading the code and shape the steps below.

- **A default-chain hostname is the proxy's, not the runtime's.** `state::decide` returns `Decision::Default` when no policy is bound, and the coordinator refuses it (`NoPolicy`). Caddy gets every TLS subject in one automation policy with the default ACME issuer (`system/caddy/config.rs`, `build_policy`), so it orders and renews over HTTP-01 on its own schedule.
- **The proxy gate (`tls.chain.proxy-gate`) is the riskiest part.** Gating Caddy off a hostname must not stop it serving a certificate it already holds, and Caddy's own renewal point is also a third of the lifetime, so it would renew exactly when a cycle starts.
  - Caddy 2.11 accepts `automatic_https.skip_certificates` and a policy `renewal_window_ratio` (checked with `caddy validate`).
  - How either behaves for a hostname that already has a certificate in the cache has not been tested.
  - The first step is a spike against a real Caddy.
- **Runtime-managed certs may not satisfy warm-cert readiness today.** `observe_warm_certs` (`system/reconcile/state.rs`) and `cert_observation::observe` look only at Caddy's on-disk cache.
  - A certificate delivered through `get_certificate` is not obviously written there.
  - `tls.canopy.serve` promises that a Canopy certificate satisfies `rt.warm_certs` like any runtime-managed one, so the observation must also consult the runtime's store.
  - This may be an existing gap for ACME-DNS and manual certificates too.
- **The report response is discarded.** `report_once` in `runtime/canopy.rs` throws away what `client.status` returns, but `StatusResponse.names` carries the held-certificate list (`certificates[].revoked`, `usable`, `key_must_be_replaced`, `key_fingerprint`).
- **The client crate has what is needed.** The lockfile pins `bestool-canopy` 0.7.10, whose `CanopyClient` has `certificates_request` (`RequestCertificateArgs { name, csr, application_type }` returning `CertificateResponse`) and `names_entitlements`. `crates/core/Cargo.toml` still says `0.7.8`; raise the minimum.
- **Two schema constraints need migrations.** `tls_policies.strategy` has `CHECK (strategy IN ('acme_dns','manual'))` (v42) and `tls_certificates.origin` has a CHECK without `canopy` (v50 rebuilt the table to add `tailscale`). The latest migration is v60.
- **Keypair and CSR generation exist.** `tls/keypair.rs` does P-256 keys and CSRs with `rcgen`, and the operator CSR flow already stores a `csr_pending` row holding the encrypted key.
- **CLI and web already have TLS surfaces.** `crates/ctl/src/tls.rs` has `Policies` subcommands and `crates/web/frontend/src/routes/Certificates.tsx` has the page, so the repo's CLI rule is met by extending them.
- **Canopy ticks are in the report loop.** `runtime/canopy.rs` `spawn` ticks every 60 seconds and `OiState` already holds `tls_coordinator`, so collection can be driven from the report path.

## Spike (blocks the proxy steps)

- [ ] Run `caddy` locally with a hostname that holds a certificate in storage (internal CA is enough) and establish, for each candidate gate:
  - [ ] whether `skip_certificates` keeps serving the cached certificate and whether the `get_certificate` manager is still consulted for the skipped name
  - [ ] whether `renewal_window_ratio` moves Caddy's renewal point, so Caddy renews only at the first fall-through point (a sixth of the lifetime remaining on a 2-strategy chain)
  - [ ] whether a gated hostname can be handed back to Caddy by a config reload alone, without losing the certificate
- [ ] Record the outcome in this plan, and pick the mechanism before the proxy steps. If neither candidate works, the fallback is for the runtime to import the proxy's cached certificate into its own store so it can serve it through `get_certificate`. That changes `tls.chain.proxy-gate`, so raise it with the user before building it.

## Baseline

- [ ] Run `cargo clippy`, `cargo fmt` and `tracey query status` and fix anything outstanding before starting
- [ ] Read `docs/failure-modes.md` (reconciler, fault store, retry loops) before the coordinator work

## Storage (new migration, v61, never edit v42 or v50)

- [ ] Rebuild `tls_policies` to allow `strategy = 'canopy'` (no provider, no cert id) and add the arm to the row CHECK; add a `.sql` file and a `Migration` entry, with a test that an existing database upgrades
- [ ] Rebuild `tls_certificates` to allow `origin = 'canopy'`, copying all columns by name, with an upgrade test
- [ ] Add a per-hostname chain row, upserted with `ON CONFLICT ... DO UPDATE` naming only its own columns, holding the cycle's start, the responsible strategy, when it became responsible and a pin for an open order
  - [ ] answer the four crash-recovery questions in the module doc (who else writes it, which restart path reads it, what each abort branch leaves, which test severs memory between write and read)
- [ ] Record revocation on the certificate row (a state or flag the serving and decision paths both honour), so a revoked certificate stops being served and stops counting as usable
- [ ] `TlsCertOrigin::Canopy` (`as_str` `"canopy"`, `parse`) and `TlsPolicy::Canopy` in `runtime/tls.rs`, with the row mapping in `tls/store.rs`; fix every exhaustive match the compiler finds (`state.rs`, `issuance.rs`, `oi/handler/tls.rs`, `expiring.rs`)

## One decision function (`tls/state.rs`)

- [ ] Extend `Snapshot` with the chain rows and the Canopy facts the decision needs: whether an offer is live, which declaring apps have a Canopy type (one set per hostname, so the request can carry or omit the application type), and whether the hostname is one a public CA can issue for. Take them as inputs, so `compute_state` stays pure
- [ ] Replace `Decision::Default` for a default-chain hostname with a chain decision: the responsible strategy, its fall-through point, and the next-cycle time, per `tls.chain.cycle`, `.ladder`, `.refusal`, `.no-certificate` and `.return`
  - [ ] cycle start: no usable certificate, or the serving certificate (any origin, including a manual upload and a proxy-held one read from the cache) at its renewal point
  - [ ] ladder: each strategy's fall-through is halfway from the moment it became responsible to the expiry of the certificate being replaced; the last has none
  - [ ] refusal passes on at once; a failure, timeout or unreachable relay never does
  - [ ] no certificate: skip an unavailable strategy at once, keep asking an available one at the debounce cadence, and keep an open order's strategy until it resolves
  - [ ] operator retry (`force_retry`) starts a cycle at once
- [ ] A Canopy-only policy is a chain of one: it is available without a live offer, never falls through, and yields the untyped-app fault condition
- [ ] The renewal point of a certificate whose expiry is unrecorded must stay "not due", as `next_renewal_at` does today
- [ ] Make `decide_acme_dns` and the new chain code share the cycle and debounce helpers instead of duplicating them
- [ ] Keep `compute_state` the only dispatch input: the coordinator, the renewal scheduler and the OI rollup read this one result

## Canopy strategy (`tls/canopy.rs`, new)

- [ ] Key handling: generate with `keypair::generate`, store as a `csr_pending` row with origin `canopy` and the key encrypted by the secret key; reuse it for every request and collection; destroy a replaced key once nothing served depends on it (`tls.canopy.key`)
- [ ] Request: one per hostname, CSR naming exactly the hostname, never a wildcard (a wildcard policy requests each concrete hostname it matches), `application_type` set only when the typed declaring apps share exactly one (`tls.canopy.request`)
- [ ] Answer classification as one function returning a type that cannot conflate "refused" with "could not ask" (`tls.canopy.outcomes`)
  - [ ] refusal: HTTP 403, or 409 for no managed zone
  - [ ] not a refusal: 409 for paused or key revoked, 412 not attached, a failed order, a relay failure, no live offer, and a malformed CSR (which also files a fault)
  - [ ] map the response `state`, `usable`, `revoked`, `key_must_be_replaced` and `last_error` onto outcomes; a state string the runtime does not recognise is an error, not pending
- [ ] Collection on the report tick: ask again while pending, and from the certificate's renewal point until a longer-lived one replaces it, and nothing in between (`tls.canopy.collect`)
- [ ] On a collected chain, store it as an active `canopy` certificate through the existing store path so supersession (`tls.cert.supersede`) retires what it replaces
- [ ] Revocation: read `names.certificates` from the report response, match on `key_fingerprint`, mark the row revoked, generate a new key, and let the chain pass the hostname on at once (`tls.canopy.revoked`)
  - [ ] `report_once` returns the entitlements instead of discarding them, and a report that fails changes nothing
- [ ] Attempt log: one entry per order, opened by the creating request, closed on collect, failure or refusal; asking again adds none (`tls.canopy.attempts`). Add an attempt trigger or reuse the existing ones, and keep `AttemptTrigger` parsing in step
- [ ] Back-off, classification and exit reporting named for every loop with a fallible await (AGENTS.md): no terminal give-up without an expiry, a fault past the threshold, and retry at the cap

## Coordinator and reconciler

- [ ] `Coordinator::run`: dispatch on the chain decision, not before it, as the Tailscale dispatch comment already requires; the Canopy and HTTP-01 steps both go through `handle_non_issuing`-style gating (retry block, debounce, force retry)
- [ ] `Coordinator::ensure` stops treating `NoPolicy` as the default for a default-chain hostname (it skips only hostnames that are neither on a chain nor bound to `acme_dns`/`canopy`)
- [ ] Hand the Canopy collection tick to the coordinator from `runtime/canopy.rs` after a report, under the same issue lock, so ticks and the reconciler cannot race on one hostname
- [ ] `site_ingress_tls_hostnames` hostnames go through the same chain (check that this matches `tls.strategy.default` for site ingresses before relying on it)

## Proxy gate (after the spike)

- [ ] Add the chosen gate to `system/caddy/config.rs` and `system/types.rs` (per-hostname "HTTP-01 responsible" input from the chain decision), emitted only for hostnames whose first strategy is not HTTP-01
- [ ] `compute_proxy_config` takes the responsible-strategy set; a hostname the runtime could not decide for (the decision errored) must not be gated or ungated, so apply nothing for it rather than reading "could not determine" as "not responsible"
- [ ] Warm-only subjects follow the same gate (`certificates.automate` is skipped while Canopy is responsible)
- [ ] Config tests in `system/caddy/tests.rs` for gated, ungated and internal-CA buckets, and that an applied config for a gated name still routes

## Observation

- [ ] `observe_warm_certs` and `cert_observation::observe` also treat a usable certificate in the runtime store covering the hostname as `cert_valid`, so ACME-DNS, manual and Canopy certificates satisfy `rt.warm_certs`; confirm first whether this is an existing gap
- [ ] A pending Canopy order is `pending` and holds the barrier, and a failed one is `failed` with the error (`observe.ingress.certs`)

## Faults

- [ ] `cert_acquisition_failed` for Canopy: file through `file_once` or `sync_faults`, keyed `(app, kind, hostname)` per declaring ingress, filed only when a request was refused or failed and the hostname has no usable certificate, or the chain has reached its last strategy with no replacement (`tls.canopy.fault`); name the filing condition, the clearing condition and the shape (condition) in a comment
- [ ] Canopy-only with no typed declaring app: the fault of `tls.canopy-only.untyped`, cleared when an app gains a type, the policy changes or clears, or no ingress declares the hostname
- [ ] Log an error wherever one of these faults is filed (AGENTS.md)
- [ ] `expiring.rs`: exempt `canopy` origin from `cert_expiring_soon`

## Interface

- [ ] `oi/handler/tls.rs`: `set_policy_canopy` for `/tls/policies/set-canopy { hostname }` (reject anything that is not an exact hostname or pattern the existing validation accepts), register it in `oi/handler.rs`, and mark `// i[tls.policy.set-canopy]`
- [ ] `list_policies` returns strategy `"canopy"`; `clear_policy` clears it
- [ ] `list_certificates` returns origin `"canopy"`
- [ ] `list_hostnames`: add the `chain` object (`responsible`, `fall_through_at?`, `next_cycle_at?`), `policy.strategy` `canopy`, `last_issuance.kind` `canopy`, and map the new decision variants in `decision_status` and `decision_next_issuance`
- [ ] `retry` starts a new cycle from the first strategy
- [ ] OI handler tests in `oi/handler/tls/tests.rs` for each of the above

## CLI and web

- [ ] `crates/ctl/src/tls.rs`: `policies set-canopy <hostname>`, and show the chain and the `canopy` origin in the `hostnames` and `certs` output
- [ ] `crates/web/frontend/src/routes/Certificates.tsx`: the Canopy-only option when binding a policy, and the hostnames view with the chain (`w[routes.certificates]`); update `Certificates.test.tsx`
- [ ] The web server proxies OI calls generically; check whether `crates/web/src/http.rs` has a per-route allowlist that needs the new route

## Dependencies and docs

- [ ] Raise the `bestool-canopy` minimum with `cargo add` (the lock already holds 0.7.10)
- [ ] `docs/deploying.md`: how default-chain hostnames behave with and without Canopy, the Canopy-only policy and when to use it, and what the operator sees in Canopy when a request arrives
- [ ] Check `docs/runtime-overview.md` and `docs/networking.md` for statements about default TLS that no longer hold

## Tracey

- [ ] `// r[impl ...]` before the specific code for each `tls.chain.*`, `tls.canopy.*` and `tls.strategy.canopy*` item, `// i[impl tls.policy.set-canopy]` on the handler, `// w[impl ...]` in the frontend, and `r[verify ...]` on tests
- [ ] `tracey query uncovered --spec-impl runtime/main` shows none of the new items uncovered
- [ ] Stubs are `todo!()`, never a silent no-op

## Tests (see `.workhorse/test-cases/j5/overview.md` once drafted)

- [ ] Pure decision tests in `state.rs`: each rule of the ladder with a clock, including 2-strategy and 1-strategy chains, ARI start versus the third, refusal, no certificate, open order, force retry
- [ ] Classification table test: every answer row of `tls.canopy.outcomes`, plus an unknown state string
- [ ] Revocation and key-replacement test across a simulated restart (state severed between write and read)
- [ ] Migration tests for v61
- [ ] Config tests for the proxy gate, and an integration check of the spike's mechanism if the harness can run Caddy

## Open

- The proxy gate mechanism, pending the spike. If neither Caddy setting holds a cached certificate while stepping aside, `tls.chain.proxy-gate` needs rewording.
- Whether warm-cert observation of runtime-managed certificates is an existing gap or already works through a path not found in this read.
