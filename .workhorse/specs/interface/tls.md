---
id: TLSM
---

# TLS certificate management

Applies to the [Operator Interface](../overview.md#operator-interface) methods through which an [operator](../overview.md#operator) manages how a [host](../overview.md#host) obtains the TLS certificates that [TLS](../runtime/tls/certificates.md) specifies.

## Borrowed terms

| term | meaning |
| --- | --- |
| ACME | the Automatic Certificate Management Environment protocol, [RFC 8555](https://www.rfc-editor.org/rfc/rfc8555) |
| DNS-01, HTTP-01 | the ACME challenge types of RFC 8555 §8.4 and §8.3 |
| ARI | ACME Renewal Information, [RFC 9773](https://www.rfc-editor.org/rfc/rfc9773) |
| CSR | a certification request in the syntax of PKCS #10, [RFC 2986](https://www.rfc-editor.org/rfc/rfc2986) |
| SAN | the subject alternative name extension of a certificate, [RFC 5280](https://www.rfc-editor.org/rfc/rfc5280) §4.2.1.6 |
| SubjectPublicKeyInfo | the public key field of a certificate, RFC 5280 §4.1.2.7 |
| PEM | the textual encoding of [RFC 7468](https://www.rfc-editor.org/rfc/rfc7468) |
| PKCS #8 | the private key information syntax of [RFC 5958](https://www.rfc-editor.org/rfc/rfc5958) |

## Terminology

### Stored certificate

A certificate with its private key, or a pending CSR with its private key, held by the [runtime](../overview.md#runtime) under an integer id.

## Conventions

A hostname, in these methods, is a string of at most 253 characters made of labels separated by dots.
Each label is 1 to 63 ASCII letters, digits and hyphens, and neither starts nor ends with a hyphen.
A hostname contains at least one ASCII letter.

Every timestamp in a request or result of these methods is an integer count of seconds since 1970-01-01T00:00:00Z, leap seconds excluded.

The runtime MUST NOT return a private key, an ACME account key or DNS provider credentials from any Operator Interface method.

If the runtime cannot read or write its TLS state while handling a request, it MUST refuse the request with `internal`.
It MUST NOT report such a failure as `not_found`.

A policy or stored certificate written through these methods MUST be taken into account by the next issuance decision the runtime makes and by the next [hostname view](#hostname-view).

## DNS providers

A DNS provider is a named entry holding the credentials the runtime uses for DNS-01 challenges, as [DNS providers](../runtime/tls/certificates.md#dns-providers) specifies.

`/tls/dns-providers/list` takes no parameters.
Its result is `{ providers }`, an array with one object per provider, ordered by name.

| member | type | meaning |
| --- | --- | --- |
| `name` | string | the provider's name |
| `kind` | string | the provider's kind |
| `created_at` | integer | when the provider was created |
| `updated_at` | integer | when the provider was last written |

`/tls/dns-providers/upsert` creates a provider, or replaces the provider of the same name.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the provider's name |
| `kind` | string | yes | the provider's kind |
| `config` | object | yes | the provider's credentials and settings, in the shape its kind defines |

The only kind is `route53`, for Amazon Route 53, whose `config` has these members.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `access_key_id` | string | yes | the access key id |
| `secret_access_key` | string | yes | the secret access key |
| `region` | string | no | the region, `us-east-1` if absent |

The runtime MUST refuse an upsert whose `kind` is not a defined kind, whose `config` does not have the shape its kind defines, or whose `name` is empty or only whitespace, with `requirements_invalid`.
The runtime MUST store `config` encrypted at rest under the [secret key](../runtime/secrets.md#secret-key).

The result is `{ ok, auto_policy_created }`, where `ok` is `true` and `auto_policy_created` says whether the upsert created the [catch-all policy](../runtime/tls/certificates.md#policies).

`/tls/dns-providers/delete` takes `name` and removes that provider.
The result is `{ "ok": true }`.
If no provider has that name, the runtime MUST refuse the request with `not_found`.
If a policy references the provider, the runtime MUST refuse the request with `requirements_invalid`.

## Policies

A policy binds a [hostname pattern](../runtime/tls/certificates.md#hostname-pattern) to a strategy for obtaining certificates.
A hostname that no policy matches follows the [default chain](../runtime/tls/chains.md#default-chain).
A policy takes effect as [the policy rules](../runtime/tls/certificates.md#policies) specify.

`/tls/policies/list` takes no parameters.
Its result is `{ policies }`, an array with one object per policy, ordered by pattern.

| member | type | meaning |
| --- | --- | --- |
| `hostname` | string | the policy's hostname pattern |
| `strategy` | string | `acme_dns` or `canopy` |
| `dns_provider` | string | the bound DNS provider, present only when `strategy` is `acme_dns` |
| `updated_at` | integer | when the policy was last written |

`/tls/policies/set-acme-dns` takes `hostname`, a hostname pattern, and `dns_provider`, a provider name.
It binds the pattern to ACME issuance over DNS-01 against that provider, as [ACME DNS-01 issuance](../runtime/tls/certificates.md#acme-dns-01-issuance) specifies, replacing any policy for the same pattern.
If `hostname` is not a hostname pattern, or no provider is named `dns_provider`, the runtime MUST refuse the request with `requirements_invalid`.
If `hostname` is an exact hostname, the runtime MUST hand it to issuance as the request completes.
The result is `{ ok, auto_issue_kicked }`, where `ok` is `true` and `auto_issue_kicked` says whether the hostname was handed to issuance.

> [!NOTE]
> Handing a hostname to issuance does not bypass the conditions under which the runtime declines to issue, such as a missing contact email or a retry block.
> A wildcard pattern names no hostname to issue for; an operator issues for each hostname it matches with [ACME-DNS issuance](#operator-driven-issuance), or lets the runtime issue as ingresses declaring matching hostnames appear.

`/tls/policies/set-canopy` takes `hostname`, a hostname pattern.
It binds the pattern to the [Canopy-only policy](../runtime/tls/canopy.md#canopy-only-policy), replacing any policy for the same pattern.
If `hostname` is not a hostname pattern, the runtime MUST refuse the request with `requirements_invalid`.
Setting the policy MUST NOT by itself replace a usable certificate a matching hostname already holds.
That certificate serves until its renewal point or an operator [retry](#operator-driven-issuance).
The result is `{ "ok": true }`.

`/tls/policies/clear` takes `hostname`, a hostname pattern, and removes the policy for that pattern.
The result is `{ "ok": true }`.
If no policy exists for the pattern, the runtime MUST refuse the request with `not_found`.

## Certificates

`/tls/certificates/list` takes no parameters.
Its result is `{ certificates }`, an array with one object per stored certificate, newest first.

| member | type | meaning |
| --- | --- | --- |
| `id` | integer | the stored certificate's id |
| `hostname` | string | a name the certificate covers, as a display label |
| `san_dns_names` | array of strings or null | the DNS names in the leaf's SAN extension |
| `requested_hostnames` | array of strings or null | the hostnames the row's CSR requested |
| `requested_not_covered` | array of strings or null | the requested hostnames the certificate does not cover |
| `renews` | integer or null | the stored certificate this row's CSR renews |
| `pending_renewal` | integer or null | the id of this certificate's pending renewal |
| `state` | string | `csr_pending`, `active`, `superseded`, `failed` or `revoked` |
| `origin` | string | `manual`, `csr`, `acme_dns`, `tailscale` or `canopy` |
| `key_type` | string | the private key's algorithm, such as `ecdsa_p256` |
| `issuer` | string or null | the leaf's issuer |
| `not_before` | integer or null | the leaf's `notBefore` |
| `not_after` | integer or null | the leaf's `notAfter` |
| `serial` | string or null | the leaf's serial number |
| `self_signed` | boolean | whether the leaf's issuer equals its subject |
| `note` | string or null | the operator's note on an upload |
| `acme_account_id` | integer or null | the ACME account that issued the certificate |
| `created_at` | integer | when the row was created |
| `updated_at` | integer | when the row was last written |

For an operator upload, `hostname` MUST be the certificate's [primary SAN](../runtime/tls/certificates.md#primary-san).
For a certificate the runtime issued, it MUST be the hostname the runtime issued for.
For a CSR awaiting its certificate, it MUST be the first requested hostname.
The hostnames a certificate serves MUST be decided by [SAN coverage](../runtime/tls/certificates.md#coverage) alone, never by `hostname`.

`san_dns_names` MUST be `null` when the row holds no certificate or its certificate cannot be parsed.
`requested_hostnames` MUST be `null` for a row that was not created by a [CSR request](#certificate-signing-requests).

> [!NOTE]
> A CA may sign a name set other than the one requested, so `requested_hostnames` can differ from the names the certificate covers.

The runtime MUST compute `requested_not_covered` under SAN coverage, so that clients need not apply the rule themselves.
It MUST be empty when the certificate covers every requested hostname.
It MUST be `null` when the row has no requested hostnames, holds no certificate, or holds a certificate that cannot be parsed.

> [!NOTE]
> An unparsable certificate yields `null` rather than every requested hostname, since the runtime cannot tell which names the certificate covers.

`renews` MUST be `null` when the row renews no certificate or the renewed certificate has been deleted.
`pending_renewal` MUST be `null` unless a CSR renewing this certificate is in state `csr_pending`.

`/tls/certificates/upload-manual` stores a certificate and private key an operator supplies.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `cert_pem` | string | yes | PEM `CERTIFICATE` blocks: the leaf and any intermediates, in any order |
| `key_pem` | string | yes | the leaf's private key, PEM-encoded PKCS #8 |
| `note` | string | no | a note for operators |

The runtime MUST accept the leaf and intermediates of `cert_pem` in any order, and MUST ignore PEM blocks of other types.
It MUST store the chain with the leaf first.

The runtime MUST refuse the upload with `requirements_invalid`, naming the failed check in the message, when:

- `cert_pem` holds no `CERTIFICATE` block, or its leaf cannot be parsed;
- `key_pem` cannot be parsed;
- the leaf has no DNS names in its SAN extension;
- the leaf's `notAfter` has passed, as [operator-supplied certificates](../runtime/tls/certificates.md#operator-supplied-certificates) specifies;
- the public key of `key_pem` differs from the leaf's SubjectPublicKeyInfo.

On success, the runtime MUST store the certificate in state `active` with origin `manual`, and the private key encrypted at rest under the secret key.
The stored certificate's `key_type` MUST name the algorithm of the uploaded key.
The certificate MUST bind to the hostnames it covers as [certificate selection](../runtime/tls/certificates.md#certificate-selection) specifies, with no per-hostname step.
It MUST retire the certificates it replaces in full, as [supersession](../runtime/tls/certificates.md#supersession) specifies.

The result is an object with these members.

| member | type | meaning |
| --- | --- | --- |
| `id` | integer | the new stored certificate's id |
| `primary_san` | string | the leaf's primary SAN |
| `san_dns_names` | array of strings | the DNS names in the leaf's SAN extension |
| `warnings` | array of strings | `self_signed` when the leaf is [self-issued](../runtime/tls/certificates.md#operator-supplied-certificates), and `not_yet_valid` when its `notBefore` is in the future |

`/tls/certificates/delete` takes `id` and removes that stored certificate.
The result is `{ "ok": true }`.
If no stored certificate has that id, the runtime MUST refuse the request with `not_found`.

## Certificate signing requests

The CSR methods carry out the flow of [certificate signing requests](../runtime/tls/certificates.md#certificate-signing-requests).

`/tls/certificates/csr/begin` generates a key pair and a CSR.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `hostnames` | array of strings | unless `renews` is given | the names to request |
| `renews` | integer | no | the stored certificate this CSR [renews](../runtime/tls/certificates.md#certificate-signing-requests) |
| `key_type` | string | no | the key algorithm, `ecdsa_p256` if absent |

Each entry of `hostnames` MUST be a hostname, or `*.` followed by a hostname.
`hostnames` MUST hold at least one and at most 100 entries.
The runtime MUST refuse a request breaking either rule with `requirements_invalid`.
The runtime MUST request once each name that differs from an earlier one only in ASCII case, keeping the first spelling.

> [!NOTE]
> 100 names is the most that public certificate authorities accept in one certificate.

If `renews` is given and `hostnames` is not, the runtime MUST request the DNS names in the renewed certificate's own SAN extension.
If it cannot read that SAN extension, or the extension holds no DNS names, it MUST refuse the request with `internal`, naming the certificate.
If neither `hostnames` nor `renews` is given, the runtime MUST refuse the request with `requirements_invalid`.

If `renews` names no stored certificate, the runtime MUST refuse the request with `not_found`.
If the named certificate is not an active certificate of origin `manual` or `csr`, or already has a pending renewal, the runtime MUST refuse the request with `requirements_invalid`.
A refusal for a pending renewal MUST name the pending renewal's id.
The runtime MUST check the renewed certificate again as it stores the new row, so that a renewal begun or the certificate retired meanwhile is refused.

If `key_type` is given and is not `ecdsa_p256`, the runtime MUST refuse the request with `requirements_invalid`.

The runtime MUST generate the key pair itself and store the private key encrypted at rest under the secret key.
The CSR's SAN extension MUST hold exactly the requested names.
The runtime MUST store a row in state `csr_pending` with origin `csr`, recording the requested names and any renewed certificate.

The result is `{ id, csr_pem }`, the new row's id and the PEM-encoded CSR.

`/tls/certificates/csr/get` takes `id` and returns `{ id, csr_pem }` for a row in state `csr_pending`.
If no stored certificate has that id, the runtime MUST refuse the request with `not_found`.
If the row is not in state `csr_pending`, the runtime MUST refuse the request with `requirements_invalid`.

`/tls/certificates/csr/upload-cert` takes `id`, a row in state `csr_pending`, and `cert_pem`, the certificate signed for its CSR in the form `upload-manual` accepts.
If no stored certificate has that id, the runtime MUST refuse the request with `not_found`.
If the row is not in state `csr_pending`, the runtime MUST refuse the request with `requirements_invalid`.

The runtime MUST check the certificate as for `upload-manual`, with the row's stored private key in place of `key_pem`.
On success, it MUST move the row to state `active`, label it with the certificate's primary SAN, and keep its requested hostnames.
The certificate MUST bind to the hostnames its own SAN extension covers, and MUST retire the certificates it replaces in full.
If the row leaves state `csr_pending` before the runtime activates it, the runtime MUST refuse the request with `requirements_invalid` and store or retire nothing.

The result is an object with these members.

| member | type | meaning |
| --- | --- | --- |
| `id` | integer | the row's id |
| `primary_san` | string | the certificate's primary SAN |
| `san_dns_names` | array of strings | the DNS names in the certificate's SAN extension |
| `requested_not_covered` | array of strings | the requested hostnames the certificate does not cover |
| `warnings` | array of strings | the warnings of `upload-manual`, and `request_not_covered` when `requested_not_covered` is not empty |

`/tls/certificates/csr/cancel` takes `id` and deletes that row with its private key.
The result is `{ "ok": true }`.
If no stored certificate has that id, the runtime MUST refuse the request with `not_found`.
If the row is not in state `csr_pending`, the runtime MUST refuse the request with `requirements_invalid`.
The runtime MUST check the state and delete the row in one atomic step, so that a certificate uploaded for the CSR while the cancellation is handled is either never activated or never deleted.

## Operator-driven issuance

`/tls/certificates/issue-acme-dns` takes `hostname` and runs ACME issuance over DNS-01 for it, returning when the issuance finishes.
The hostname's policy MUST be an `acme_dns` policy, matched as [policy matching](../runtime/tls/certificates.md#policies) specifies.
The issuance MUST use the contact email and certificate profile of the [settings](#settings).
It MUST proceed even when the hostname's certificate is not due for renewal, or the runtime is waiting out the [after-failure interval](../runtime/tls/certificates.md#after-failure-interval).
It MUST be logged as an [issuance attempt](#issuance-attempts) triggered by `manual`.
The issued certificate MUST retire the certificates it replaces in full.

The result is `{ cert_id, not_after }`, the new certificate's id and `notAfter`.
The runtime MUST refuse the request with `requirements_invalid`, with a message naming the cause, when the hostname has no `acme_dns` policy, when a [retry block](#retry-blocks) is set for it, or when no contact email is configured.
The runtime MUST answer with `internal` when the issuance fails.
A message for a failed issuance MUST name the stage that failed.

`/tls/certificates/retry` takes `hostname` and queues a fresh attempt to obtain its certificate.
The runtime MUST clear any retry block for the hostname, record a persistent retry request as [retries](../runtime/tls/certificates.md#retry-blocks-and-retries) specifies, and hand the hostname to issuance.
The retry MUST start a new cycle of the hostname's chain from its first strategy, as [return to the first strategy](../runtime/tls/chains.md#return-to-the-first-strategy) specifies.
The runtime MUST return before the issuance runs.
The result is `{ "ok": true }`.
A retry request recorded before the runtime stops MUST be acted on after it restarts.

## Hostname view

`/tls/hostnames/list` returns the per-hostname rollup of [hostname state](../runtime/tls/certificates.md#hostname-state).
It takes `app`, optional, an app name.
Its result is `{ hostnames }`, an array with one object per hostname in the rollup's scope, ordered by hostname.
When `app` is given, the array MUST hold only the hostnames that app declares, and each `apps` member MUST name only that app.

| member | type | meaning |
| --- | --- | --- |
| `hostname` | string | the hostname |
| `apps` | array of strings | the apps declaring the hostname, ordered by name |
| `policy` | object | the policy resolved for the hostname |
| `status` | string | the hostname's status |
| `chain` | object or null | the hostname's position in its strategy chain |
| `active_cert` | object or null | the certificate serving the hostname |
| `last_issuance` | object or null | how the serving certificate was obtained |
| `last_error` | string or null | the error of the hostname's latest attempt, when that attempt failed |
| `retry_block` | object or null | `{ set_at, reason }` when a retry block is set |
| `force_retry_at` | integer or null | when a queued operator retry was requested |
| `next_issuance_at` | integer or null | when the runtime next expects to issue |
| `next_issuance_source` | string or null | how `next_issuance_at` was derived |

`policy` MUST be `{ "strategy": "default" }` when no policy matches the hostname.
Otherwise it MUST carry these members.

| member | type | meaning |
| --- | --- | --- |
| `strategy` | string | `acme_dns` or `canopy` |
| `dns_provider` | string | the bound DNS provider, present only for `acme_dns` |
| `pattern` | string | the matching policy's hostname pattern |
| `is_wildcard_match` | boolean | whether `pattern` differs from `hostname` |

`status` MUST be one of these values.

| `status` | meaning |
| --- | --- |
| `active` | a certificate serves the hostname and its `notAfter` has not passed |
| `expired` | the certificate serving the hostname is past its `notAfter` |
| `pending` | the runtime is obtaining a certificate the hostname lacks, or acting on a queued operator retry |
| `error` | the runtime cannot obtain a certificate for the hostname until something changes, or is waiting out the after-failure interval |
| `blocked` | a retry block is set for the hostname |
| `default` | none of the above: the hostname follows its chain with nothing serving it and nothing in progress |

Where the meanings of several values hold at once, `status` MUST be the first of them in this order: `blocked`, `expired`, `error`, `pending`, `active`, `default`.

The runtime MUST derive `status` from the same decision its issuance acts on, as [hostname state](../runtime/tls/certificates.md#hostname-state) requires.
For a hostname with no runtime-held certificate, `status` MUST reflect a certificate the proxy holds for it, so that a proxy-held certificate past its `notAfter` reports `expired`.

`chain` MUST be `null` for a hostname whose policy names a single strategy.
Otherwise it MUST carry these members.

| member | type | meaning |
| --- | --- | --- |
| `responsible` | string | `canopy`, `acme_http01` or `internal_ca` |
| `fall_through_at` | integer or null | when the responsible strategy passes to the next, if it will |
| `next_cycle_at` | integer or null | when the next cycle starts, as [chain cycles](../runtime/tls/chains.md#cycles) defines |

While a cycle runs, `responsible` MUST name the strategy acting in it.
While no cycle runs, `responsible` MUST name the strategy that will act when the next cycle starts.

`active_cert` MUST describe the stored certificate serving the hostname, or else a certificate the proxy holds for it, or be `null` when there is neither.

| member | type | meaning |
| --- | --- | --- |
| `id` | integer or null | the stored certificate's id, `null` for a proxy-held certificate |
| `origin` | string | the stored certificate's origin, or `caddy` for a proxy-held certificate |
| `caddy_issuer` | string | for a proxy-held certificate only: `local` for the proxy's internal CA, otherwise the host of the ACME directory that issued it |
| `issuer` | string or null | the leaf's issuer |
| `not_before` | integer or null | the leaf's `notBefore` |
| `not_after` | integer or null | the leaf's `notAfter` |
| `self_signed` | boolean | whether the leaf's issuer equals its subject |
| `ari_window_start` | integer or null | the start of the ARI suggested renewal window |
| `ari_window_end` | integer or null | the end of the ARI suggested renewal window |

`last_issuance` MUST be `null` when the runtime knows of no issuance of the serving certificate.
Otherwise it MUST carry these members.

| member | type | meaning |
| --- | --- | --- |
| `kind` | string | the serving certificate's origin, or `caddy` for a proxy-held certificate |
| `at` | integer | when the certificate was obtained |
| `cert_id` | integer or null | the stored certificate's id, `null` for a proxy-held certificate |
| `provider` | string or null | the DNS provider for `acme_dns`, the `caddy_issuer` value for `caddy`, and otherwise `null` or absent |

The runtime MUST NOT derive `last_issuance` from an issuance attempt whose certificate is no longer stored.

`next_issuance_source` MUST be `null` when the runtime expects no issuance, and otherwise one of these values.

| value | meaning |
| --- | --- |
| `ari` | `next_issuance_at` is the start of the certificate's ARI suggested window |
| `fallback` | `next_issuance_at` is the [renewal point](../runtime/tls/certificates.md#renewal-point) given by the one-third rule |
| `immediate` | issuance is due as soon as the runtime acts, and `next_issuance_at` is `null` |
| `debounce` | `next_issuance_at` is when the after-failure interval ends |

## Issuance attempts

`/tls/certificates/attempts/list` returns the log of [issuance attempts](../runtime/tls/certificates.md#attempt-log), newest first.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `hostname` | string | no | return only attempts for this hostname |
| `limit` | integer | no | the most entries to return, from 1 to 500, 100 if absent |

The runtime MUST refuse a `limit` below 1 or above 500 with `requirements_invalid`.

Its result is `{ attempts }`, an array of objects with these members.

| member | type | meaning |
| --- | --- | --- |
| `id` | integer | the attempt's id |
| `hostname` | string | the hostname the attempt was for |
| `triggered_by` | string | `on_demand`, `manual` or `renewal` |
| `started_at` | integer | when the attempt started |
| `finished_at` | integer or null | when the attempt finished, `null` while it runs |
| `outcome` | string | `pending`, `success` or `failure` |
| `cert_id` | integer or null | the certificate a successful attempt produced |
| `error` | string or null | why a failed attempt failed |

## Retry blocks

A retry block pauses the runtime's issuance for a hostname, as [retry blocks](../runtime/tls/certificates.md#retry-blocks-and-retries) specifies.

`/tls/retry-blocks/list` takes no parameters.
Its result is `{ blocks }`, an array with one object per block, ordered by hostname.

| member | type | meaning |
| --- | --- | --- |
| `hostname` | string | the blocked hostname |
| `set_at` | integer | when the block was set |
| `set_by` | string | `auto` for a block the runtime set after a failed issuance, `operator` for one an operator set |
| `reason` | string or null | why the block was set |

`/tls/retry-blocks/set` takes `hostname` and `reason`, an optional string, and sets an operator block for the hostname, replacing any block it has.
The result is `{ "ok": true }`.
The runtime MUST keep an operator block until an operator clears it with `/tls/retry-blocks/clear` or [retries](#operator-driven-issuance) the hostname.

`/tls/retry-blocks/clear` takes `hostname` and removes the hostname's block, if it has one.
The result is `{ ok, cleared }`, where `ok` is `true` and `cleared` says whether a block was removed.

## Settings

`/tls/settings/get` takes no parameters and returns the host's TLS settings.

| member | type | meaning |
| --- | --- | --- |
| `contact_email` | string | the [contact email](../runtime/tls/certificates.md#acme-accounts-and-settings) for ACME account registration, empty when none is configured |
| `cert_profile` | string or null | the [ACME profile](../runtime/tls/certificates.md#acme-accounts-and-settings) requested on every order, `null` when none is configured |
| `updated_at` | integer | when the settings were last written |

`/tls/settings/set` changes the settings it is given and leaves the others unchanged.

| member | type | required | meaning |
| --- | --- | --- | --- |
| `contact_email` | string | no | the new contact email; the empty string clears it |
| `cert_profile` | string or null | no | the new ACME profile; `null` clears it, so that the CA selects its default profile |

The result is `{ "ok": true }`.
A change MUST take effect from the runtime's next issuance or renewal, without a restart.
