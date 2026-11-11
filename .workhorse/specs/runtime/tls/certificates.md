---
id: TLS
---

# TLS certificates

## TLS Certificate Strategy

<!-- from runtime: TLS Certificate Strategy -->

The runtime obtains and serves TLS certificates for hostnames declared by [TLS-terminating ingresses](rule:l/ingress.tls).
Operators may, via the operator interface, override how a given hostname's certificate is obtained on a per-hostname basis.
This section defines the available strategies and their contracts.
The BSL surface is intentionally strategy-agnostic: scripts declare only that an ingress wants TLS for a hostname, never how the certificate is acquired.

> r[tls.strategy.default]
> When no operator-defined strategy applies to a hostname declared by a TLS-terminating ingress, its certificate comes from the first strategy in the default chain that is available for it.
> The chain is [Canopy](rule:r/tls.strategy.canopy), then a public ACME certificate using the HTTP-01 challenge.
> Canopy is in the chain only for a hostname it [applies to](rule:r/tls.strategy.canopy); for any other public hostname the chain is HTTP-01 alone.
> This is the default and applies to any hostname not bound to a different strategy.
> For hostnames that no public CA can issue for — IP literals, single-label hostnames, and the `.localhost`, `.local`, `.internal` TLDs (the same set excluded from wildcard auto-binding in [tls.policy.wildcard](rule:r/tls.policy.wildcard)) — the chain is the proxy's internal CA alone, producing a self-signed certificate that the proxy serves directly without contacting any external authority.
> Canopy is never asked for such a hostname.
> A stored certificate that covers the hostname is not a step in the chain: it serves for as long as it ranks first under [tls.strategy.manual](rule:r/tls.strategy.manual), and the chain runs when it needs replacing, per [tls.chain.cycle](rule:r/tls.chain.cycle).

> r[tls.strategy.acme-dns]
> Operators may bind a hostname or wildcard pattern to a [DNS provider](rule:r/tls.dns-provider.lifecycle); the runtime must drive ACME for any matching hostname using the DNS-01 challenge against the bound provider.
> The runtime must perform the ACME flow itself rather than configuring the proxy to do so, so that DNS-provider credentials are not exposed to the proxy or its persistent configuration.
> Every challenge record the runtime publishes for an issuance must be removed once the CA no longer needs it, whether the issuance succeeded or failed at any step, so that no record is left behind in the operator's zone.

> r[tls.policy.wildcard]
> Policies may name a hostname pattern instead of an exact hostname.
> Two pattern shapes must be supported: the catch-all `*`, and a left-anchored shell-glob-style subdomain wildcard `*.<suffix>` matching any hostname that ends in `.<suffix>` (and is not equal to `<suffix>` itself).
> The wildcard covers multiple subdomain levels: `*.example.com` matches both `foo.example.com` and `a.b.example.com`. This is *not* the RFC 6125 DNS-wildcard semantic (which would match exactly one extra label); the runtime is matching operator policy, not a certificate's SAN, so the broader semantic lets a single rule cover an entire zone.
> When several patterns match a hostname, the most-specific match wins: an exact pattern beats any wildcard, a longer `*.<suffix>` beats a shorter one, and `*` is least specific.
> Operators that want to override a sub-zone may add a more-specific pattern (e.g. `*.internal.example.com` or `host.example.com`) alongside.
> Wildcard patterns must not auto-bind hostnames that the proxy itself manages with an internal CA — IP literals, single-label hostnames, and the `.localhost`, `.local`, `.internal` TLDs — because no public CA can issue for them and DNS-01 has nowhere to put the challenge record.
> An exact (non-wildcard) policy still applies to such hostnames so an operator can deliberately override.

> r[tls.policy.auto-default]
> When the operator configures the first DNS provider on a node and no `*` policy already exists, the runtime must automatically create a catch-all `*` policy bound to that provider with the `acme_dns` strategy.
> The auto-policy is operator-mutable: it may be cleared, re-bound, or replaced like any other policy, and it must not be re-created on subsequent provider additions.

> r[tls.settings.contact-email]
> The runtime must persist a single global operator contact email used whenever it registers an ACME account on the operator's behalf.
> The setting must be readable and writable through the operator interface, take effect on the next renewal pass without restart, and default to empty.
> Issuance and renewal that need to register a new ACME account must fail with a clear error when no contact email is configured.

> r[tls.settings.cert-profile]
> The runtime must persist an optional global ACME profile name and forward it on every issuance order via the ACME profiles extension.
> When unset, the runtime must omit the profile field so the CA selects its default profile (typically a ~90-day certificate at Let's Encrypt).
> When set, every subsequent ACME-DNS order — first issuance, renewal, and operator-driven retry — must include that profile name; the CA chooses validity and any other profile-defined attributes accordingly. Let's Encrypt's `shortlived` profile, for example, yields ~6-day certificates, and the runtime's existing renewal threshold (1/3 of remaining lifetime, or RFC 9773 ARI when supplied) handles the resulting shorter renewal cadence without further configuration.
> The setting must be readable and writable through the operator interface and take effect on the next renewal pass without restart.

> r[tls.cert.attempt-log]
> The runtime must record every certificate-issuance attempt — successful or failed — in a durable log, scoped per hostname.
> Each entry must capture the hostname, the trigger (`on_demand`, `manual`, or `renewal`), start and finish timestamps, the outcome (`pending`, `success`, or `failure`), the resulting certificate id (on success), and the error string (on failure).
> The operator interface must expose this log so that an unsuccessful issuance is visible without having to inspect daemon logs.

> r[tls.cert.retry-block]
> Operators may set a per-hostname retry block to pause runtime-driven issuance.
> While a block is set, the issuance coordinator must skip the hostname.
> The block persists across restarts and is removed only by explicit operator action (clear or operator-driven retry).

> r[tls.cert.eager-issuance]
> The runtime — not the proxy — must drive every certificate-issuance attempt for runtime-managed strategies.
> The reconciler must hand each TLS-terminating ingress hostname to the issuance coordinator on every tick.
> The coordinator must dedup in-flight requests, skip hostnames whose policy is neither `acme_dns` nor `canopy` and that are not on the default chain, skip hostnames that already have an active certificate and are not due under [tls.chain.cycle](rule:r/tls.chain.cycle), debounce after a recent failure, and run the rest in the background.
> A hostname counts as in flight until every issuance running for it has finished, whether the reconciler or an operator started it, so an operator-triggered issuance finishing never lets the reconciler start a duplicate alongside a background one still running.
> The cert-serving endpoint must remain a pure lookup: it must never trigger an issuance flow itself (see [tls.cert.serve](rule:r/tls.cert.serve)).

> r[tls.cert.force-retry]
> Operator-driven retry must be expressible as a persistent state row keyed by hostname, so the request survives a daemon restart between the operator clicking retry and the reconciler picking it up.
> The issuance coordinator must consume the row atomically at the start of an issuance run, bypassing the recent-failure debounce.
> A successful issuance must remove the row; subsequent operator-driven retries write a fresh one.

> r[tls.acme.account.persist]
> The runtime must persist ACME account state — at minimum the account private key and the URL returned by the directory's newAccount endpoint — for each ACME directory the runtime issues against, encrypted at rest using the [secret key](rule:r/secret.key).
> All issuance against a given directory must reuse a single persisted account rather than creating a new one per strategy, hostname, or contact-email change.

> r[tls.acme.account.contact-update]
> When the operator changes [tls.settings.contact-email](rule:r/tls.settings.contact-email) and a persisted account already exists for the directory, the runtime must update the contact information on the existing account (RFC 8555 §7.3.2) rather than registering a new account.
> The persisted contact email must be updated to match only after the directory accepts the change; if the directory rejects the update, the runtime must continue using the existing account, log the failure, and leave the persisted email unchanged so a later attempt can retry.

> r[tls.acme.renewal.auto]
> The runtime must autonomously renew certificates it has issued via ACME (DNS-01 strategy) before they expire.
> Renewal must be attempted while the certificate is still valid, with sufficient lead time that a transient failure does not cause expiry.
> Successful renewal must replace the prior certificate atomically so that in-flight TLS handshakes are unaffected.

> r[tls.cert.ari]
> When the issuing CA advertises ACME Renewal Information (RFC 9773), the runtime should use the CA's suggested renewal window to schedule renewal in preference to its own fixed-fraction-of-lifetime heuristic.
> The runtime must persist the suggested window alongside the certificate (start, end, and the timestamp at which it was last polled) and treat the certificate as due for renewal once the current time has reached the window's start.
> When submitting a renewal order for a certificate whose predecessor's RFC 9773 identifier (authority key identifier plus serial) is available, the runtime must include that identifier in the order's `replaces` field so the CA can correlate the renewal.
> If the CA does not support ARI or the lookup fails, the runtime must fall back to its fixed-fraction-of-lifetime renewal heuristic without surfacing the absence as an error.

> r[tls.strategy.manual]
> Operators may upload a PEM-encoded certificate chain and matching private key.
> The runtime must auto-bind the uploaded cert to every hostname its SubjectAlternativeName list covers — literally for exact entries, and per RFC 6125 single-label rules for wildcard SANs — and cause the proxy to serve that exact pair for TLS handshakes whose SNI matches a covered hostname.
> Auto-binding requires no per-hostname operator action: a `*.example.com` cert covers `foo.example.com` and `bar.example.com` as soon as it is uploaded; further hostnames added later are picked up automatically.
> When more than one stored cert covers the same hostname, precedence goes first to a certificate inside its validity window, then to one that is not self-issued over one that is, then to one whose SAN list names the hostname exactly over one that only covers it by wildcard (RFC 6125 §6.4.4), and only then to the most recently created.
> Self-issuance ranks above specificity because a certificate clients reject is no use for the hostname however precisely it names it: a wildcard they accept must be able to take over from a dedicated certificate they do not.
> Self-issued means the leaf's issuer and subject are the same; the runtime builds no chain and consults no trust store, so this distinguishes a certificate an operator self-signed from one some CA issued, and says nothing about whether that CA is one any client trusts.
> A wildcard accordingly serves every name it covers that has no better certificate of its own, so a wildcard obtained for one hostname is picked up by the others it covers without further operator action.
> Were trust not ranked here at all, resolution would serve a certificate that [supersession](rule:r/tls.cert.supersede) had just refused to let retire anything, and the refusal would buy nothing.
> The runtime does not auto-renew manual certs on its own; however, if an `acme_dns` policy applies to a covered hostname and the manual cert is past its renewal threshold, the runtime must initiate the normal ACME-DNS issuance flow so a renewable cert can take over before the manual cert expires.

> r[tls.csr.flow]
> Operators may instruct the runtime to generate a keypair and a Certificate Signing Request for one or more hostnames, so that a single certificate can be requested for several names.
> The runtime must:
>
> - Generate the keypair on the server, store the private key encrypted at rest using the [secret key](rule:r/secret.key), and never expose it via any operator interface.
> - Produce a PEM-encoded CSR whose Subject Alternative Name set is exactly the requested hostnames, and make the CSR retrievable via the operator interface for as long as the request is pending.
> - Record the hostnames the request was made for and retain them once a certificate arrives, so an operator can see what was asked for alongside what was issued.
> - Accept a signed certificate uploaded later, provided it matches the stored private key and passes the same validation as a manual upload.
> - Bind that certificate to the hostnames its own SAN set covers, per [SAN coverage](rule:r/tls.cert.validation.san-coverage). A CA is free to sign a name set other than the one requested, so the requested hostnames do not determine what the accepted certificate serves or what it supersedes.
> - Report an unmet request name by name: when the issued certificate does not cover one or more of the requested hostnames, the upload must warn that the request was not met and say which hostnames were missed, and each missed hostname must go on appearing in the [per-hostname rollup](rule:r/tls.cert.hostname-view) as having no active certificate for as long as an ingress declares it.
> - Permit cancellation of a pending CSR by the operator, which must destroy the stored private key.

> r[tls.csr.renew]
> Operators may begin a CSR as the renewal of an active certificate the runtime cannot renew by itself, which is a manual upload or a CSR-derived certificate; a certificate of any other origin or state cannot be renewed this way.
> Unless the operator names the hostnames, the renewal requests exactly the DNS names in the renewed certificate's own SAN set, wildcards included, and not the hostnames that certificate's own CSR requested.
> A CA may sign a name set other than the one requested, typically widening a single name to a wildcard; the issued set is one that CA has already signed, and a wildcard in it goes on covering hostnames added after the renewal. An operator renewing through a CA that will not sign the wildcard again names the hostnames instead.
> The renewal records the certificate it renews.
> A certificate has at most one pending renewal: beginning another while one is pending is refused, and uploading or cancelling the pending one ends it.
> When the renewal's certificate is uploaded, it retires the certificate it renews if it replaces it in full per [tls.cert.supersede](rule:r/tls.cert.supersede), whichever names the two certificates are labelled by.
> Deleting the renewed certificate while its renewal is pending leaves the renewal pending, with nothing recorded for it to retire.

> r[tls.dns-provider.lifecycle]
> The runtime must provide named, separately addressable DNS-provider entries that hold the credentials needed for DNS-01 challenges.
> Provider credentials must be stored encrypted at rest using the [secret key](rule:r/secret.key).
> Multiple hostnames may reference the same provider entry.
> Deletion of a provider entry must be refused while any hostname's strategy references it.

> r[tls.cert.metadata]
> For every hostname declared by a TLS-terminating ingress, the runtime must surface to the operator interface: the active strategy, the certificate's issuer (when known), `notBefore`, `notAfter`, and the acquisition status as defined in [observe.ingress.certs](rule:r/observe.ingress.certs).
> Metadata for certificates the proxy obtained itself (ACME HTTP-01 and its internal CA) is derived from the proxy's certificate cache; metadata for runtime-managed certificates (Canopy, ACME DNS-01, manual, CSR) is derived from the runtime's own certificate store.

> r[tls.cert.hostname-view]
> The runtime must expose a per-hostname rollup, keyed by the set of TLS-terminating ingress hostnames currently declared by apps the runtime is actively managing — i.e. apps in any phase except `NotInstalled` — combining for each hostname the originating app(s), the resolved policy, the active certificate (if any), the latest issuance attempt's outcome and error, any operator retry block or queued operator retry, and an expected next-issuance time.
> The set of in-scope hostnames must be computed from a single shared enumeration over the app registry; the issuance coordinator (driven by the reconciler), the expiring-cert fault sweep, and the operator interface rollup must all consult the same function so they cannot disagree about which hostnames the runtime owns. A hostname declared only by a `NotInstalled` app must not appear in any of them.
> The rollup must be a *projection* of the same per-hostname decision the issuance coordinator and renewal scheduler use to decide what to act on; the operator interface, the reconciler-driven coordinator, and the renewal task must all consult one decision function over one DB snapshot, so what the operator sees and what the runtime does are structurally identical rather than separately maintained.
> The next-issuance time accordingly follows the ARI suggested window when present (see [tls.cert.ari](rule:r/tls.cert.ari)), otherwise the same lifetime-fraction fallback the coordinator applies.
> The rollup must support filtering to a single app so the same view can be embedded on per-app surfaces without re-implementing the rollup logic.

> r[tls.cert.serve]
> For runtime-managed certificates (ACME DNS-01, manual, and CSR-derived), the runtime must deliver certificate and key material to the ingress proxy through a mechanism that does not require including private key material in the proxy's persistent configuration or its restart-replay cache.
> The proxy must be able to obtain the appropriate certificate by SNI hostname at TLS handshake time.
> The serving endpoint must be a pure lookup: a stored cert returns 200 with PEM, an unknown hostname returns 204 (no content), and the runtime must never trigger an issuance flow from this path.
> The cost of a lookup must grow with the stored certificates that could cover the requested hostname rather than with every certificate stored, and a hostname no stored certificate could cover must be answered without examining any certificate's contents.
> Lookups share the one thread all of the runtime's database work runs on, and certificate history is kept indefinitely, so a lookup whose cost tracked the whole store would slow reconcile, the operator interface and issuance along with it.
> A hostname whose stored certificate cannot be read, because the store could not be queried or the key it holds cannot be decrypted, returns 204 as well: the proxy reads anything else as the handshake failing, so an error answer would stop the hostname serving altogether for as long as the condition held. Issuance is the issuance coordinator's job (see [tls.cert.eager-issuance](rule:r/tls.cert.eager-issuance)).
> A certificate outside its validity window must not be served: neither one whose `notAfter` has passed, nor one whose `notBefore` has not yet arrived, and neither may take precedence over a stored certificate that is currently valid and covers the hostname.
> Storing a certificate ahead of its `notBefore` is how an operator stages a cutover (see [tls.cert.validation.expired](rule:r/tls.cert.validation.expired)), so serving it early would defeat the reason it was accepted.
> A stored certificate whose expiry is unrecorded is not treated as expired, since that cannot be distinguished from one that has not been parsed.

> r[tls.cert.validation.san-coverage]
> A certificate covers a hostname when the leaf certificate's Subject Alternative Name DNS entries either contain that hostname literally or contain a wildcard entry that covers it under RFC 6125.
> A wildcard SAN `*.example.com` covers exactly one additional left-most label (it covers `foo.example.com` but not `example.com` and not `a.b.example.com`).
> Coverage is the only thing that binds a certificate to a hostname.
> However a certificate reached the runtime — operator upload, externally-signed CSR, or ACME issuance — it must be served for exactly the hostnames its own SAN set covers.
> The name an operator asked for is a record of the request, never a binding: a certificate whose SAN set omits it does not acquire that hostname by having been requested under it.
> A certificate carrying no DNS SANs covers nothing and must be rejected on operator upload.

> r[tls.cert.supersede]
> A certificate supersedes another only when it replaces it in full: it [covers](rule:r/tls.cert.validation.san-coverage) every hostname the other serves, it is within its own validity window and lasts at least as long as the other, and it is not self-issued unless the one it replaces already was.
> A certificate staged ahead of its own validity window is not replaced at all: it was stored for a cutover, and retiring it would mean the cutover never happens with nothing left to take over.
> The self-issuance test is the leaf's issuer against its subject, not a chain built to a trust store, so it stops an operator's self-signed upload retiring a CA-issued certificate and does not stop a certificate from an untrusted CA doing so.
> A certificate that does not meet the bar retires nothing, and whatever is serving a hostname goes on serving it.
> This matters most on the CSR path, where the SAN set is chosen by the issuing CA rather than by the operator, so which certificates a new one is even a candidate to replace is outside the operator's control.
> Where the runtime cannot read back the certificate it has just stored, it must report the failure rather than a count of nothing retired: the two are not the same outcome, and treating them alike leaves a replaced certificate active alongside its replacement with nothing said.

> r[tls.cert.validation.self-signed]
> The runtime must accept a self-signed leaf certificate (issuer DN equal to subject DN, no chain) on operator upload, but must annotate the stored certificate so that the operator interface can flag it.

> r[tls.cert.validation.expired]
> The runtime must reject an operator-supplied certificate whose `notAfter` is already in the past at upload time, and must accept (with annotation) a certificate whose `notBefore` is in the future.

> r[tls.fault.expiring]
> For certificates that the runtime cannot autonomously renew (manual uploads and CSR-derived certificates), if `notAfter` is within fourteen days, the runtime must file a fault of kind `cert_expiring_soon` against each ingress that uses the affected hostname, identifying the hostname, the strategy, the expiry timestamp, and the certificate, and directing the operator to [renew it](rule:r/tls.csr.renew).
> ACME-issued certificates are exempt — the proxy is responsible for autonomous renewal.
> Canopy-issued certificates are exempt too: renewal is the chain's job, and a chain that cannot replace one is reported by [tls.canopy.fault](rule:r/tls.canopy.fault).
> The fault is cleared automatically when a non-expiring certificate replaces the expiring one, or when no ingress references the hostname.
