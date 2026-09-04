# Facility app must not force an HTTPS ingress

## What the card turned out to be

The card as written asked for a change to `apps/tamanu-facility.seed.rhai`: make
`public-hostname` optional and guard the ingress declaration the way central does. That is only a
small part of the work.

The facility definition **already** guarded its ingress on `is_set()`, mirroring central. Only
`.required(true)` on the param remained, and the definitions here are demo artefacts; the
production Tamanu definition is separate and pending. So the real question is the one behind the
card: **does anything in Seedling prevent a plaintext `.local` host being served by a site
ingress with no app ingress?**

Answer: yes, one thing, and it is decisive.

## Audit: what is already correct

Seedling does not force HTTPS on `.local`. Every cert and redirect path was checked and behaves:

- `.local`, `.localhost`, `.internal`, IP literals and single-label names are partitioned into
  the internal-CA bucket (`is_caddy_internal`, `crates/core/src/runtime/tls/state.rs:299`).
  `crates/core/src/system/caddy/config.rs:112` carries a comment explaining why the automation
  policy must pin the internal issuer rather than let the unpinned ACME chain fail on names
  public CAs cannot issue for.
- A site ingress with `tls_provider: none` yields `tls = false`
  (`crates/core/src/system/reconcile/site_proxy.rs:221`).
- Site ingresses never synthesise an HTTP to HTTPS redirect (`redirect: None`, same file), and
  BSL `redirect()` refuses without HTTPS.
- TLS automation policies cover only `tls_acme` vhosts plus warm hostnames, so a plaintext vhost
  requests no certificate at all.
- `rt.warm_certs` filters to TLS-terminating ingresses and an empty selection is immediately
  satisfied (`docs/spec/language.md:1016`), so cert warming cannot block install on a host that
  declares no ingress.
- Hostname validation accepts `.local`; there is no public-suffix requirement.

## The blocker

`crates/core/src/system/translate/proxy.rs:181` conflates "terminates HTTP" with "terminates
HTTPS":

```rust
let is_https = ingress.http_terminate.is_some();
```

This held while only app ingresses existed. BSL can only produce `http_terminate: Some(..)`
together with `tls: true`, because the sole HTTP combinations are `Terminate.Https +
Output.Http1/Http2` (`crates/core/src/defs/ingress.rs:124`). Site ingresses broke the invariant:
`site_proxy.rs` sets `http_terminate: Some(term)` for any HTTP or HTTP2 attachment while taking
`tls` from the parent's TLS provider. `register_listeners` never reads `.tls`.

Measured consequence for `clinic.local`, `tls_provider: none`, HTTP attachment on :80 forwarding
to an app service:

```
listeners: [{80, Https}, {80, Quic}]        no plain HTTP listener
vhost:     clinic.local tls_acme=false routes=1
caddy:     {"apps": {"http": {"servers": {}}}}
```

Nothing is emitted. The route is dropped on both sides: `seedling_https` is skipped because it
filters on `tls_acme`, and `seedling_http` is skipped because no HTTP listener exists. A spurious
QUIC listener is registered on the plaintext port. A plaintext `.local` host is silently
unserved.

Secondary, same conflation, currently app-only so not live:
`crates/core/src/system/reconcile/rules.rs:178` sets `ForwardProto::Both` when
`http_terminate.is_some()`, which would open UDP DNAT for a plaintext HTTP ingress.

## Vhost identity

`ensure_vhost` keys `VirtualHost` by hostname alone and ORs `tls_acme` across every ingress for
that hostname. A hostname with a plaintext :80 attachment and a TLS :443 one collapses into a
single vhost whose `tls_acme` is true, so the plaintext route is served from the HTTPS server.
Conflict detection does not catch this because it keys on `(hostname, port)` and the two ports
differ. Decision: key vhosts by `(hostname, tls)` so each is served from the listener matching
its own termination.

## Demo definitions

Both Tamanu definitions now demonstrate the no-ingress deployment, so the possibility is visible
in the demo even before the proxy fix lands. These files illustrate; they are not the production
definition and are not covered by the test suite.

- `public-hostname` is `.required(false)` in both, with the description saying that leaving it
  unset declares no ingress so a site ingress carries the traffic
- The `canonical` closure throws a legible message when neither `public-hostname` nor
  `canonical-url` is set, since with no public hostname there is no HTTPS ingress to derive an
  advertised URL from. The check sits inside the closure, not at the top level: the closure only
  runs from `render_config` (install and `on_change`), by which point params are supplied, whereas
  a top-level throw would fire during first registration and leave the script unloadable
- `canonical-url`'s description records that it is required when `public-hostname` is unset

Verified by evaluating both files through `evaluate_script` across all four param combinations
(probe since removed): each evaluates without error, an ingress resource is declared only when the
hostname is set, and invoking `canonical` with both unset throws the intended message.

## Build steps

- [x] Add `r[actuate.ingress.plaintext]` to `docs/spec/runtime.md` and cross-reference it from
      `r[ingress.site.attachment]`
- [x] Make `public-hostname` optional in both demo definitions, with the `canonical` guard
- [ ] `register_listeners`: derive the listener protocol from `ingress.tls`, not from
      `http_terminate`. A plaintext HTTP ingress registers an HTTP listener on its port and no
      QUIC listener
- [ ] `rules.rs`: derive `ForwardProto` from what the ingress actually serves, so a plaintext
      HTTP ingress forwards TCP only
- [ ] Key `VirtualHost` by `(hostname, tls)` in `ensure_vhost`, so a plaintext and a
      TLS-terminating vhost for one hostname stay separate
- [ ] Check the downstream consumers of `ProxyConfig.virtual_hosts` still hold once a hostname
      can appear twice: `routed_subjects` and `augment_with_warm_certs` both filter on
      `tls_acme`, so neither should need changing, but confirm
- [ ] Annotate the implementation with tracey `r[impl actuate.ingress.plaintext]` references
- [ ] Regression tests per the test cases, generic BSL and site-ingress shapes only, with the
      Tamanu topology as sample data at most
- [ ] `cargo clippy`, `cargo fmt`, `tracey query status`
