---
id: TLSM
---

# TLS certificate management

## TLS Certificate Management

<!-- from interface: TLS Certificate Management -->

The TLS certificate operator surface mirrors the runtime concepts defined in [the runtime spec](runtime.md): named DNS providers, per-hostname policies, and a certificate registry covering manual uploads, CSR-derived certs, and certs the runtime obtained itself via ACME or Canopy.
This section covers the operator interface for the ACME-DNS strategy, manual cert lifecycle, and policy management.

## DNS Providers

<!-- from interface: TLS Certificate Management / DNS Providers -->

> i[tls.dns-provider.list]
> `/tls/dns-providers/list` returns the configured DNS providers without their credentials.
> Response `result.providers` is an array of objects each with `name` (string), `kind` (string), `created_at`, and `updated_at` (Unix timestamp integers).

> i[tls.dns-provider.upsert]
> `/tls/dns-providers/upsert { name, kind, config }` creates a new provider entry or replaces an existing one with the same name.
> `config` is a provider-specific object whose shape depends on `kind`. For `kind = "route53"` the required fields are `access_key_id`, `secret_access_key`, and an optional `region` (defaults to `us-east-1`).
> The credentials are stored encrypted at rest and never returned through any operator endpoint.
> Returns `requirements_invalid` if `name` is empty or `kind` is unknown.
> When this is the first DNS provider on the node and no `*` policy already exists, the runtime auto-creates a catch-all `*` policy bound to it per [tls.policy.auto-default](rule:r/tls.policy.auto-default); the response field `auto_policy_created` indicates whether that happened.

> i[tls.dns-provider.delete]
> `/tls/dns-providers/delete { name }` removes a DNS provider entry.
> Returns `not_found` if no provider with that name exists.
> Returns `requirements_invalid` if any policy still references the provider; the operator must clear those policies first.

## Policies

<!-- from interface: TLS Certificate Management / Policies -->

> i[tls.policy.list]
> `/tls/policies/list` returns all per-hostname policy rows.
> Response `result.policies` is an array of objects each with `hostname`, `strategy` (`"acme_dns"`, `"canopy"`, or `"manual"`), and either `dns_provider` (for acme-dns) or `cert_id` (for manual), plus `updated_at`.
> Hostnames not in this list use the default chain per [tls.strategy.default](rule:r/tls.strategy.default).

> i[tls.policy.set-acme-dns]
> `/tls/policies/set-acme-dns { hostname, dns_provider, directory_url? }` binds `hostname` to the named DNS provider for ACME-DNS-01 issuance.
> `hostname` may be an exact hostname, the catch-all `*`, or a shell-glob-style subdomain wildcard such as `*.example.com` (which matches `foo.example.com`, `a.b.example.com`, and any deeper subdomain), per [tls.policy.wildcard](rule:r/tls.policy.wildcard).
> The new policy takes effect on the next reconciliation tick per [tls.policy.apply](rule:r/tls.policy.apply).
> When `hostname` is exact and the global [contact email](rule:i/tls.settings.set) is configured, the runtime fires a single ACME-DNS issuance attempt asynchronously if no active cert exists yet; the response includes `auto_issue_kicked: true` to signal that.
> Wildcard policies do not auto-issue (there is no concrete hostname to acquire a cert for); the operator runs [`tls.cert.issue-acme-dns`](rule:i/tls.cert.issue-acme-dns) per hostname.
> `directory_url` defaults to the Let's Encrypt production directory.

> i[tls.policy.set-canopy]
> `/tls/policies/set-canopy { hostname }` binds `hostname` to the Canopy-only policy per [tls.strategy.canopy-only](rule:r/tls.strategy.canopy-only).
> `hostname` may be an exact hostname, the catch-all `*`, or a shell-glob-style subdomain wildcard, with the matching rules of [tls.policy.wildcard](rule:r/tls.policy.wildcard).
> The new policy takes effect on the next reconciliation tick per [tls.policy.apply](rule:r/tls.policy.apply).
> Setting it does not by itself replace a certificate the hostname already holds: a usable certificate is left until its renewal point, or until an operator [retry](rule:i/tls.cert.retry).

> i[tls.policy.clear]
> `/tls/policies/clear { hostname }` removes any operator policy for `hostname`, returning it to the default chain per [tls.strategy.default](rule:r/tls.strategy.default).
> Returns `not_found` if no policy exists for the hostname.

## Certificates

<!-- from interface: TLS Certificate Management / Certificates -->

> i[tls.cert.list]
> `/tls/certificates/list` returns all stored certificates.
> Response `result.certificates` is an array of objects each with `id`, `hostname`, `san_dns_names`, `requested_hostnames`, `requested_not_covered`, `renews`, `pending_renewal`, `state` (`"csr_pending"`, `"active"`, `"superseded"`, `"failed"`, or `"revoked"`), `origin` (`"manual"`, `"csr"`, `"acme_dns"`, or `"canopy"`), `key_type`, `issuer`, `not_before`, `not_after`, `serial`, `self_signed`, `note`, `acme_account_id`, `created_at`, `updated_at`.
> `hostname` is a name the certificate covers, used as a display label: for an operator upload its primary SAN, for a runtime-issued certificate the name it was issued for. What the certificate serves is decided by its full SAN list per [tls.cert.validation.san-coverage](rule:r/tls.cert.validation.san-coverage), never by this field.
> `requested_hostnames` is the set of hostnames the row's CSR requested, null for a row that was not issued against a CSR the runtime generated; it may differ from what the certificate covers, since the CA may sign a different name set.
> `requested_not_covered` lists those of the requested hostnames the stored certificate does not cover, empty when it covers them all, so that the rule is applied in one place rather than by each client. It is null where there is nothing to decide — no requested hostnames, or no certificate yet — and also where the stored certificate cannot be parsed, which is not the same answer as "covers none of them".
> `san_dns_names` is the DNS names in the stored certificate's SAN set, null where there is no certificate yet or it cannot be parsed.
> `renews` is the id of the certificate the row's CSR [renews](rule:r/tls.csr.renew), null when it renews none or that certificate has since been deleted; `pending_renewal` is the id of the row's own renewal while that renewal is pending, otherwise null.
> Private key material is never returned.

> i[tls.cert.upload-manual]
> `/tls/certificates/upload-manual { cert_pem, key_pem, note? }` registers an operator-supplied certificate-and-key pair and returns `{ id, primary_san, san_dns_names, warnings }`.
> The `cert_pem` may carry a leaf with optional intermediates in any PEM order (the runtime re-emits the chain leaf-first); the `key_pem` is a PKCS#8 PEM private key.
> The runtime validates the upload per [tls.cert.validation.self-signed](rule:r/tls.cert.validation.self-signed) and [tls.cert.validation.expired](rule:r/tls.cert.validation.expired), additionally rejects the upload when the leaf carries no DNS SANs (nothing to bind to), and rejects when the supplied private key's public key does not match the leaf certificate's `SubjectPublicKeyInfo`.
> Validation rejections return `requirements_invalid` with a message describing the failed rule.
> On success the cert is stored with the private key encrypted at rest using the [secret key](rule:r/secret.key); the response `warnings` array contains `"self_signed"` when the leaf is self-signed and/or `"not_yet_valid"` when `not_before` is in the future.
> No per-hostname binding step is required: the runtime auto-binds the cert to every hostname its SAN list covers per [tls.strategy.manual](rule:r/tls.strategy.manual). The new row retires any prior active certificate it fully replaces, per [tls.cert.supersede](rule:r/tls.cert.supersede).

> i[tls.cert.delete]
> `/tls/certificates/delete { id }` removes a stored certificate.
> Refused with `requirements_invalid` when a manual policy still references the row, so the operator unbinds the policy first; returns `not_found` when the id is unknown.

> i[tls.cert.csr.begin]
> `/tls/certificates/csr/begin { hostnames?, renews?, key_type? }` generates a server-side keypair and a CSR whose SAN set is `hostnames`, and returns `{ id, csr_pem }`.
> `hostnames` is a non-empty array of at most a hundred names, each entry a hostname or a `*.<suffix>` wildcard; a name repeated is requested once, and anything else is refused with `requirements_invalid`.
> `renews` is the id of a certificate this CSR [renews](rule:r/tls.csr.renew); when it is given and `hostnames` is not, `hostnames` is that certificate's SAN DNS names.
> `hostnames` is required when `renews` is not given.
> `renews` returns `not_found` when the id is unknown, and `requirements_invalid` when the certificate is not an active manual or CSR-derived one, or already has a pending renewal, whose id the message names.
> The cap is what public certificate authorities accept in one certificate, and it keeps a single request from storing and serving back a certificate request of unbounded size.
> The private key is stored encrypted at rest using the [secret key](rule:r/secret.key) and never leaves the runtime; only the CSR is returned, both in this response and via subsequent `tls.cert.csr.get` calls while the row remains in `csr_pending`.
> `key_type` defaults to `"ecdsa_p256"`, which is currently the only accepted value.

> i[tls.cert.csr.get]
> `/tls/certificates/csr/get { id }` returns `{ id, csr_pem }` for a pending CSR row.
> Returns `not_found` when the id is unknown, and `requirements_invalid` once the row has been transitioned away from `csr_pending` (cert uploaded or row cancelled).

> i[tls.cert.csr.upload-cert]
> `/tls/certificates/csr/upload-cert { id, cert_pem }` accepts the externally-signed certificate for a pending CSR, returning `{ id, primary_san, san_dns_names, requested_not_covered, warnings }`.
> The runtime verifies the leaf cert's `SubjectPublicKeyInfo` matches the stored private key and applies the same validation as [`tls.cert.upload-manual`](rule:i/tls.cert.upload-manual).
> On success the row transitions to `active`, binds to the hostnames the issued certificate's SAN list covers per [tls.cert.validation.san-coverage](rule:r/tls.cert.validation.san-coverage), and retires any prior active certificate it fully replaces, per [tls.cert.supersede](rule:r/tls.cert.supersede).
> A CSR that is cancelled before its certificate is uploaded cannot then be activated: the upload returns `requirements_invalid` and nothing is stored or retired.
> The hostnames the CSR was requested for are retained on the row and reported by [`tls.cert.list`](rule:i/tls.cert.list).
> `requested_not_covered` lists the requested hostnames the issued certificate does not cover, empty when it covers them all.
> Validation rejections return `requirements_invalid`; non-fatal warnings come back in the response `warnings` array: `self_signed`, `not_yet_valid`, and `request_not_covered` when the issued certificate does not cover every requested hostname.

> i[tls.cert.csr.cancel]
> `/tls/certificates/csr/cancel { id }` cancels a pending CSR row, deleting the stored private key.
> Refused with `requirements_invalid` when the row is no longer `csr_pending`; for active certs use [`tls.cert.delete`](rule:i/tls.cert.delete) instead.
> The state is checked as part of the deletion, so a certificate uploaded for the CSR while the cancellation is in flight is either never activated or never deleted: a cancellation that loses the race to an upload is refused and the certificate stays active.

> i[tls.cert.issue-acme-dns]
> `/tls/certificates/issue-acme-dns { hostname }` synchronously runs the ACME-DNS-01 issuance flow for `hostname` via the issuance coordinator.
> The hostname must resolve (per [tls.policy.wildcard](rule:r/tls.policy.wildcard)) to an `acme_dns` policy with a configured DNS provider; otherwise the call returns `internal` with a descriptive message.
> Contact email and directory come from the global [TLS settings](rule:i/tls.settings.set); if no contact email is configured, the call returns `internal` with a descriptive message.
> The call blocks for the full duration of the ACME flow (typically tens of seconds) and returns `{ cert_id, not_after }` on success.
> Failure returns `internal` with a message identifying the stage that failed.
> The newly-issued certificate retires any prior active certificate it fully replaces, per [tls.cert.supersede](rule:r/tls.cert.supersede).

> i[tls.cert.retry]
> `/tls/certificates/retry { hostname }` queues a retry for `hostname` and returns immediately.
> The runtime clears any operator pause for the hostname, writes a persistent force-retry signal per [tls.cert.force-retry](rule:r/tls.cert.force-retry), and nudges the issuance coordinator.
> The retry starts a new cycle of the hostname's chain from its first strategy, per [tls.chain.return](rule:r/tls.chain.return), so a hostname that passed to a later strategy asks Canopy again.
> The actual issuance runs in the background; the cert appears in [`tls.cert.list`](rule:i/tls.cert.list) once issuance completes.
> Survives a daemon restart: if the daemon dies between this call and the coordinator picking it up, the next reconciler tick after restart still processes it.

## Hostname view

<!-- from interface: TLS Certificate Management / Hostname view -->

> i[tls.hostname.list]
> `/tls/hostnames/list { app? }` returns a per-hostname rollup covering every TLS-terminating ingress hostname currently declared by a registered app.
> When `app` is supplied, only hostnames declared by that app are returned.
> Response `result.hostnames` is an array; each entry has:
>
> - `hostname`
> - `apps`: array of app names declaring this hostname (multiple when several apps share an ingress)
> - `policy`: `{ strategy: "default" | "acme_dns" | "canopy" | "manual", dns_provider?, cert_id?, pattern?, is_wildcard_match? }` — `default` means no policy is bound and the hostname follows the default chain; `pattern` is the matched policy pattern (which may be a wildcard), `is_wildcard_match` is true when the resolved policy was a wildcard rather than an exact match
> - `status`: `"active" | "expired" | "error" | "pending" | "blocked" | "no_cert" | "default"` — derived from policy, active certificate, last attempt, and any retry block or queued retry. For `default`-strategy hostnames, `status` reflects the proxy's on-disk certificate when one is available (so a Caddy-served cert near expiry surfaces as `expired` rather than indistinguishably as `default`).
> - `chain`: `{ responsible, fall_through_at?, next_cycle_at? }` — the strategy currently responsible for the hostname (`"canopy"`, `"acme_http01"`, or `"internal_ca"`), when it falls through to the next strategy in Unix seconds if it will, and when the next cycle starts per [tls.chain.cycle](rule:r/tls.chain.cycle). Where no cycle is running, the strategy named is the one that will act when the next one starts. Null for a hostname on no chain, which is one bound to a policy naming a single strategy
> - `active_cert`: `{ id, origin, caddy_issuer?, issuer, not_before, not_after, self_signed, ari_window_start, ari_window_end }` or null. `id` is null for proxy-managed certs; `origin` is `"caddy"` for those, with `caddy_issuer` naming the proxy's issuer subdir (`local` for its internal CA, otherwise the host of the ACME directory).
> - `last_issuance`: `{ kind, at, cert_id?, provider? }` describing when and how the active certificate was last obtained, or null when no issuance has run; `kind` is the origin of the certificate that issuance produced (`"manual"`, `"csr"`, `"acme_dns"`, `"tailscale"` or `"canopy"`, with `provider` set to the DNS provider for `acme_dns`), or `"caddy"` for proxy-managed certs (with `provider` set to the issuer subdir). An issuance whose certificate is no longer stored says nothing about how the active certificate was obtained, so it is not reported.
> - `last_error`: error message from the most recent failed attempt for this hostname, when the latest attempt was a failure, otherwise null
> - `retry_block`: `{ set_at, reason }` when an operator pause is set, otherwise null
> - `force_retry_at`: timestamp of a queued operator-driven retry awaiting the coordinator, otherwise null
> - `next_issuance_at`: expected next issuance time in Unix seconds, or null
> - `next_issuance_source`: `"ari" | "fallback" | "immediate" | "debounce"` describing how `next_issuance_at` was derived (RFC 9773 ARI window, the runtime's lifetime-fraction fallback, "as soon as the coordinator runs", or "the time the after-failure debounce expires"), or null when the strategy is manual or the certificate is one the proxy obtained for itself
>
> The view is the canonical operator surface for what TLS hostnames exist and what state they are in; it deliberately replaces the older "list every stored cert" and "list every recent attempt" views, which exposed implementation rather than per-hostname state.

## Cert attempt log

<!-- from interface: TLS Certificate Management / Cert attempt log -->

> i[tls.cert.attempts.list]
> `/tls/certificates/attempts/list { hostname?, limit? }` returns the recent issuance-attempt log.
> Newest first; capped at `limit` (default 100).
> Each entry has `id`, `hostname`, `triggered_by` (`on_demand` | `manual` | `renewal`), `started_at`, `finished_at` (null while pending), `outcome` (`pending` | `success` | `failure`), `cert_id` (set on success), and `error` (set on failure).
> When `hostname` is supplied, scopes the result to that hostname.

## Retry blocks

<!-- from interface: TLS Certificate Management / Retry blocks -->

> i[tls.retry-block.list]
> `/tls/retry-blocks/list` returns the current per-hostname retry blocks.
> Response `result.blocks` is an array of `{ hostname, set_at, set_by, reason }`.
> `set_by` is `auto` for runtime-imposed blocks following an issuance failure, and `operator` for blocks set explicitly via [`tls.retry-block.set`](rule:i/tls.retry-block.set).

> i[tls.retry-block.set]
> `/tls/retry-blocks/set { hostname, reason? }` sets an operator retry block for `hostname`.
> While the block is set, on-demand ACME-DNS issuance for the hostname is skipped per [tls.cert.retry-block](rule:r/tls.cert.retry-block).
> An operator block is not auto-cleared; only a manual call to [`tls.retry-block.clear`](rule:i/tls.retry-block.clear) (or a successful manual issuance via [`tls.cert.issue-acme-dns`](rule:i/tls.cert.issue-acme-dns)) removes it.

> i[tls.retry-block.clear]
> `/tls/retry-blocks/clear { hostname }` removes the retry block for `hostname` if any.
> Returns `{ cleared: bool }` indicating whether a block was removed.

## Settings

<!-- from interface: TLS Certificate Management / Settings -->

> i[tls.settings.get]
> `/tls/settings/get` returns the global TLS settings as `{ contact_email, cert_profile, updated_at }`. Empty `contact_email` means none is configured. `cert_profile` is null when no profile has been set, otherwise the configured ACME profile name.

> i[tls.settings.set]
> `/tls/settings/set { contact_email?, cert_profile? }` updates whichever fields are present.
> `contact_email` (string) sets the operator contact email used for ACME account registration; pass an empty string to clear.
> `cert_profile` (string or null) opts into a non-default ACME profile (e.g. Let's Encrypt's `shortlived` for ~6-day certs) per [tls.settings.cert-profile](rule:r/tls.settings.cert-profile); pass `null` to clear so the CA selects its default profile, or omit the field to leave it unchanged.
> Both updates take effect on the next renewal pass without daemon restart.
