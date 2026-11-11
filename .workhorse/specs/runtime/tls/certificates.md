---
id: TLS
---

# TLS certificates

Applies to how the [runtime](../../overview.md#runtime) obtains, stores, selects and serves certificates for the hostnames of TLS-terminating ingresses.

## Borrowed terms

| term | meaning |
| --- | --- |
| ACME, ACME account, directory, order | as defined by [RFC 8555](https://www.rfc-editor.org/rfc/rfc8555) |
| DNS-01, HTTP-01 | the ACME challenge types of RFC 8555 §8.4 and §8.3 |
| renewal information, suggested window, certificate identifier | as defined by [RFC 9773](https://www.rfc-editor.org/rfc/rfc9773) §4.1 and §4.2 |
| ACME profile | a named set of issuance parameters a CA offers, selected by an order's `profile` field, as defined by [draft-ietf-acme-profiles](https://datatracker.ietf.org/doc/draft-ietf-acme-profiles/) |
| subject alternative name (SAN), validity window, `notBefore`, `notAfter` | as defined by [RFC 5280](https://www.rfc-editor.org/rfc/rfc5280) §4.2.1.6 and §4.1.2.5 |
| self-issued | a certificate whose subject and issuer fields carry the same distinguished name, per RFC 5280 §6.1 |
| certificate signing request (CSR) | a PKCS #10 certification request, per [RFC 2986](https://www.rfc-editor.org/rfc/rfc2986) |
| PEM | the textual encoding of [RFC 7468](https://www.rfc-editor.org/rfc/rfc7468) |
| SNI | the TLS Server Name Indication extension, per [RFC 6066](https://www.rfc-editor.org/rfc/rfc6066) §3 |

## Terminology

### Managed hostname

A hostname declared by a TLS-terminating ingress, per [ingress termination](../../language/ingress.md#termination), of an [app](../../overview.md#app) whose status is anything other than `NotInstalled`.

### Internal name

A hostname that is an IP address literal, has a single label, or is `localhost`, `local` or `internal` or ends in one of those labels.
No public CA issues certificates for an internal name.

### Strategy

One way of obtaining a certificate for a hostname.
The strategies are Canopy, defined in [TCAN](canopy.md); ACME HTTP-01 and the proxy's internal CA, both carried out by the ingress proxy; and ACME DNS-01 and [Tailscale](#tailscale-certificates), both carried out by the runtime.

### Policy

An [operator](../../overview.md#operator)'s binding of a hostname or hostname pattern to a strategy.

### Runtime-managed certificate

A certificate, with its private key, that the runtime stores and serves to the ingress proxy.

### Proxy-held certificate

A certificate the ingress proxy obtained itself through ACME HTTP-01 or its internal CA.

### Certificate label

The hostname a runtime-managed certificate is stored under: for an operator upload, the first DNS name in its SAN set; for any other certificate, the hostname it was obtained for.

### Renewal point

The moment a certificate becomes due for replacement: the start of the suggested window its CA supplied through renewal information, or otherwise the moment one third of its validity window remains.

### After-failure interval

One hour, the least time the runtime waits after a failed or refused attempt before trying the same strategy for the same hostname again.

## Origins

Every runtime-managed certificate records its origin.

| origin | meaning |
| --- | --- |
| `manual` | uploaded by an operator with its private key |
| `csr` | signed externally against a CSR the runtime generated |
| `acme_dns` | issued to the runtime through ACME DNS-01 |
| `canopy` | issued by Canopy against a key the runtime holds |
| `tailscale` | obtained from the host's Tailscale facility, as [Tailscale certificates](#tailscale-certificates) specifies |

The runtime cannot renew a `manual` or `csr` certificate by itself.

## Policies

A managed hostname that no policy matches MUST obtain its certificate through the [default chain](chains.md#default-chain).
A definition declares only that an ingress terminates TLS for a hostname, never which strategy obtains its certificate.

The policies an operator can bind are ACME DNS-01 through a named DNS provider, and the [Canopy-only policy](canopy.md#canopy-only-policy).

A policy names an exact hostname, the catch-all pattern `*`, or a subdomain pattern `*.<suffix>`.
The runtime MUST refuse a policy whose name is none of these, or whose hostname or suffix is not a valid DNS name.
An exact pattern MUST match a hostname equal to it, ignoring ASCII case.
The pattern `*` MUST match every hostname.
The pattern `*.<suffix>` MUST match every hostname that ends in `.<suffix>` and is longer than it, at any depth: `*.example.com` matches `foo.example.com` and `a.b.example.com`, and not `example.com`.

> [!NOTE]
> Policy patterns deliberately differ from certificate wildcards, which cover one label only (see [coverage](#coverage)).
> A policy is operator configuration, and one pattern is meant to cover a whole zone.

Where several policies match a hostname, the runtime MUST apply the most specific: an exact pattern before any wildcard, a longer `*.<suffix>` before a shorter one, and `*` last.
A wildcard pattern MUST NOT match an internal name.
An exact policy for an internal name MUST apply.

When an operator adds a DNS provider while none exists and no `*` policy exists, the runtime MUST create a `*` policy binding ACME DNS-01 through that provider, in the same transaction as the provider.
The runtime MUST NOT create that policy on any later addition of a provider.
An operator MAY change or remove the created policy as any other.

A policy change MUST take effect within the reconciliation that follows it, without any further operator step.
The runtime MUST rebuild the proxy configuration to match.

## DNS providers

A DNS provider entry is a named set of credentials the runtime uses to publish DNS-01 challenge records.
The runtime MUST store provider credentials encrypted at rest with the [secret key](../secrets.md#secret-key), and MUST NOT return them through any interface.
Several policies MAY name the same provider.
The runtime MUST refuse to delete a provider that any policy names.

The provider kinds and their credentials are listed in [TLSM](../../interface/tls.md).

## ACME accounts and settings

The runtime issues ACME DNS-01 certificates against the Let's Encrypt production directory, `https://acme-v02.api.letsencrypt.org/directory`.

The runtime MUST persist, for each directory it issues against, one ACME account: the account's private key and the account URL the directory returned.
It MUST store the account private key encrypted at rest with the secret key.
All issuance against a directory MUST use that one account, whatever the strategy, hostname or contact email.

The runtime MUST persist one contact email for the host, empty until an operator sets it.
While the contact email is empty, the runtime MUST NOT start an ACME DNS-01 issuance, and MUST report each issuance it declines for that reason as the `cert_issue_failed` fault of [ACME DNS-01 issuance](#acme-dns-01-issuance).
The runtime MUST register a new account with the contact email as a `mailto:` contact.

If the contact email differs from the one recorded for an existing account, the runtime MUST update the account's contacts at the directory, per RFC 8555 §7.3.2, before its next issuance against that directory.
The runtime MUST record the new email against the account only once the directory has accepted the update.
If the directory rejects the update, the runtime MUST go on using the account, MUST log the failure, and MUST leave the recorded email unchanged so that the next issuance tries again.

The runtime MUST persist an optional ACME profile name for the host.
While a profile name is set, every ACME DNS-01 order the runtime places MUST name it in the order's `profile` field.
While none is set, the runtime MUST omit the field.

> [!NOTE]
> Let's Encrypt's `shortlived` profile issues certificates valid for about six days.
> The renewal point is a fraction of each certificate's own validity window, so a shorter profile renews proportionally sooner without further configuration.

A change to the contact email or the profile MUST take effect for the next issuance, without a restart.
The operator surface for these settings is specified in [TLSM](../../interface/tls.md).

## ACME DNS-01 issuance

For a hostname bound to an ACME DNS-01 policy, the runtime MUST carry out the ACME flow itself, publishing challenge records through the policy's DNS provider.
The runtime MUST NOT give DNS provider credentials to the ingress proxy or write them into its configuration.

The runtime MUST publish each challenge record at the name RFC 8555 §8.4 specifies, which for a hostname `*.<base>` is the record name of `<base>`.
The runtime MUST remove every challenge record it published for an issuance once the CA no longer needs it, whether the issuance succeeded or failed at any step.

The runtime MUST generate a new key pair for each issuance, and SHOULD generate an ECDSA key on curve P-256.

The runtime MUST NOT run two ACME orders at once.

If an issuance fails, the runtime MUST file a fault of kind `cert_issue_failed` against the hostname, carrying the failure, and MUST log it.
The runtime MUST NOT file a second such fault for a hostname that already has one.
It MUST clear the fault when an issuance for the hostname succeeds.

## Tailscale certificates

A hostname that a [Tailscale-discovered site ingress](../site-resources.md#tailscale-discovery) names, while that site ingress is not stale and its TLS provisioning mode is `tailscale`, MUST obtain its certificate from the host's Tailscale facility, whatever policy matches it.
The runtime MUST NOT request a certificate for such a hostname from a public certificate authority.

The runtime MUST store a certificate obtained this way, with its private key, as a runtime-managed certificate of origin `tailscale`.
It MUST obtain a fresh certificate once the stored one reaches its renewal point.
Obtaining one MUST pass the same [retry gate](#retry-blocks-and-retries) as any other issuance, and a failure MUST file and clear `cert_issue_failed` as for [ACME DNS-01 issuance](#acme-dns-01-issuance).

## Renewal

For a hostname under an ACME DNS-01 policy, the runtime MUST renew the hostname's certificate once it reaches its renewal point.
A renewal MUST replace the earlier certificate without interrupting service: until the new certificate is stored, the earlier one MUST go on being served, and handshakes in progress MUST NOT fail because of the replacement.

When a hostname under an ACME DNS-01 policy is served a certificate of any other origin, including a `manual` or `csr` certificate, the runtime MUST start ACME DNS-01 issuance for it once that certificate reaches its renewal point.

After issuing a certificate, the runtime SHOULD fetch its renewal information if the directory offers it.
While the runtime uses renewal information for a certificate, it MUST fetch it again on the schedule of RFC 9773 §4.3.
The runtime MUST persist, with the certificate, the start and end of the suggested window and the time it last fetched the information.
A certificate MUST become due at the start of its suggested window.
If the directory offers no renewal information, or fetching it fails, the runtime MUST fall back to the one-third rule of the renewal point, and MUST NOT report the absence as a fault.

When the certificate being replaced was issued through the same ACME account and the directory offers renewal information, the order MUST name that certificate's identifier in its `replaces` field, per RFC 9773 §5.
Otherwise the order MUST omit the field.

## Issuance

The runtime, and never the ingress proxy, MUST decide when a runtime-managed certificate is obtained.
On every reconciliation, the runtime MUST consider each managed hostname for issuance.

The runtime MUST NOT start an issuance for a hostname while another for it is running, whether reconciliation or an operator started either.
A hostname MUST count as having an issuance running until every issuance started for it has finished.

The runtime MUST NOT start an issuance for a hostname that has a certificate before that certificate reaches its renewal point, unless an operator asks for one.
A covering certificate staged to begin within the after-failure interval MUST count as the hostname's certificate for this purpose.
A certificate staged to begin further ahead MUST NOT.

After a failed attempt, the runtime MUST NOT start another for the same hostname until the after-failure interval has passed, unless an operator asks for one.

The decision for each managed hostname, covering whether to issue now, when it is next due, and which strategy is responsible, MUST be made once, from one reading of the stored state.
The issuance driven by reconciliation, the renewal of due certificates, the [operator's view](#hostname-state) and the [expiring-certificate fault](#expiring-certificates) MUST all act on that decision.
The set of managed hostnames MUST likewise be computed once and shared by all of them.

## Retry blocks and retries

An operator MAY set a retry block on a hostname.
While a block is set, the runtime MUST NOT start runtime-driven issuance for the hostname, whether by ACME DNS-01 or Canopy, and MUST refuse an operator's direct ACME DNS-01 issuance for it.
A block MUST persist across restarts.
The runtime MUST remove a block only when an operator clears it or asks for a retry of the hostname.

An operator retry of a hostname MUST be recorded durably before the request that asked for it returns, so that it survives a restart.
The runtime MUST consume a recorded retry exactly once, in the same transaction as the issuance or [cycle](chains.md#cycle) it starts.
A retry MUST bypass the after-failure interval.

## Attempt log

The runtime MUST record every issuance attempt it makes, successful or not, in a durable log, one entry per attempt.

| field | meaning |
| --- | --- |
| hostname | the hostname the attempt was for |
| trigger | `on_demand` for issuance the runtime started itself, `manual` for an operator's direct issuance, `renewal` for a renewal |
| started, finished | when the attempt began, and when it ended |
| outcome | `pending` while it runs, then `success` or `failure` |
| certificate | the certificate stored, on success |
| error | the failure, on failure |

The log is exposed through the operator interface, per [TLSM](../../interface/tls.md).

## Coverage

A certificate covers a hostname when a DNS name in its SAN set equals the hostname, ignoring ASCII case, or is a wildcard that matches it under [RFC 9525](https://www.rfc-editor.org/rfc/rfc9525) §6.3.
A wildcard `*.example.com` covers exactly one further label: it covers `foo.example.com`, and covers neither `example.com` nor `a.b.example.com`.

Coverage alone MUST decide which hostnames a certificate is served for.
The runtime MUST serve a certificate for exactly the hostnames its own SAN set covers, however it obtained the certificate.
A hostname that was requested for a certificate, or that the certificate is labelled with, MUST NOT be served the certificate unless the certificate covers it.

## Certificate selection

Where several active runtime-managed certificates cover a hostname, the runtime MUST select one by these criteria, each deciding only between certificates equal on every earlier one:

1. one inside its validity window, over one outside it;
2. one that is not self-issued, over one that is;
3. one whose SAN set names the hostname exactly, over one that covers it only by a wildcard;
4. the one most recently stored.

> [!NOTE]
> Self-issuance ranks above an exact name because a certificate clients reject is of no use however precisely it names the hostname.
> A wildcard clients accept takes over from a dedicated certificate they would reject.
> It also keeps selection in line with [supersession](#supersession), which never lets a self-issued certificate retire one that is not.

The self-issued test MUST compare the leaf certificate's issuer and subject, and nothing else.
The runtime MUST NOT build a chain or consult a trust store to rank certificates.

A runtime-managed certificate needs no policy to be served: every hostname it covers MUST be able to select it from the moment it is stored, including hostnames declared later.

## Supersession

When the runtime stores a certificate, it MUST retire each other active certificate that the new one replaces in full, in the same transaction as storing it.
The candidates are the active certificates with the same certificate label as the new one, and, for a [renewal CSR](#certificate-signing-requests), the certificate it renews.

The new certificate replaces a candidate in full only when all of these hold:

- the new certificate covers every DNS name in the candidate's SAN set;
- the new certificate is inside its validity window;
- the candidate is not staged ahead of its own validity window;
- both expiries are recorded, and the new certificate's is no earlier than the candidate's;
- the new certificate is not self-issued, or the candidate is self-issued too.

A candidate that does not meet the bar MUST stay active.
A candidate whose certificate the runtime cannot read MUST stay active.

> [!NOTE]
> On the CSR path the issuing CA chooses the SAN set, so which certificates a new one may replace is outside the operator's control.
> The bar keeps such a certificate from retiring one that still serves names the new one does not.

If the runtime cannot read back the certificate it has just stored, it MUST fail the store rather than report that nothing was retired.

## Operator-supplied certificates

An operator MAY upload a certificate chain in PEM with its private key.
The runtime MUST reject an upload whose leaf certificate carries no DNS name in its SAN set.
It MUST reject an upload whose private key does not match the leaf certificate's public key.
It MUST reject an upload whose leaf `notAfter` has passed.
It MUST accept a leaf whose `notBefore` is in the future, and MUST annotate it as not yet valid.
It MUST accept a self-issued leaf, and MUST annotate it so that the operator interface can flag it.

The runtime MUST store the private key encrypted at rest with the secret key, and MUST NOT return it through any interface.

> [!NOTE]
> Storing a certificate ahead of its `notBefore` is how an operator stages a cutover.

## Certificate signing requests

An operator MAY ask the runtime to generate a key pair and a CSR for one or more hostnames.
The runtime MUST generate the key pair itself, MUST store the private key encrypted at rest with the secret key, and MUST NOT expose the private key through any interface.
The CSR's SAN set MUST be exactly the requested hostnames.
The runtime MUST make the CSR retrievable through the operator interface while the request is pending.
It MUST record the requested hostnames, and keep them after a certificate arrives.

The runtime MUST accept a certificate later uploaded for a pending request, provided its public key matches the stored key and it passes the checks on [operator-supplied certificates](#operator-supplied-certificates).
It MUST serve that certificate for the hostnames it covers, which need not be the hostnames requested.
If the certificate does not cover every requested hostname, the upload MUST warn that the request was not met and name each hostname missed.
A missed hostname MUST go on showing as having no active certificate for as long as an ingress declares it and nothing else covers it.

An operator MAY cancel a pending request.
Cancelling MUST destroy the stored private key.
The runtime MUST check the request is still pending in the same step as deleting it, so that a cancellation racing an upload either deletes a request that never activates or is refused.

An operator MAY begin a request as the renewal of an active `manual` or `csr` certificate.
The runtime MUST refuse a renewal of a certificate of any other origin or state.
Unless the operator names hostnames, the renewal MUST request exactly the DNS names in the renewed certificate's SAN set, wildcards included.
A renewal MUST record the certificate it renews.
A certificate MUST have at most one pending renewal, and the runtime MUST refuse to begin another while one is pending.
Uploading or cancelling the pending renewal ends it.

When a renewal's certificate is uploaded, it MUST retire the renewed certificate if it replaces it in full under [supersession](#supersession), whatever labels the two carry.
Deleting the renewed certificate while its renewal is pending MUST leave the renewal pending, with no certificate recorded for it to retire.

> [!NOTE]
> A CA may sign a different name set from the one requested, often widening a single name to a wildcard.
> Renewing from the issued SAN set asks for a set the CA has already signed, and a wildcard in it goes on covering hostnames added later.
> An operator renewing through a CA that will not sign the wildcard again names the hostnames instead.

## Serving

The runtime MUST deliver runtime-managed certificates and their keys to the ingress proxy on request, at handshake time, selected by the SNI hostname.
The runtime MUST NOT write private key material into the proxy's persistent configuration or any state the proxy replays on restart.
The runtime MUST compare the SNI hostname to managed hostnames ignoring ASCII case and any trailing dot.

A lookup MUST only read stored state, and MUST NOT start an issuance.
A lookup MUST answer with the [selected](#certificate-selection) certificate when one covers the hostname, and otherwise that it has none.
A lookup MUST NOT answer with a certificate outside its validity window.
A certificate whose expiry is unrecorded MUST NOT be treated as expired.

If the stored certificate for a hostname cannot be read, because the store cannot be queried or the key cannot be decrypted, the lookup MUST answer that it has none, and MUST log the failure.

> [!NOTE]
> The proxy treats any answer other than a certificate or "none" as a failed handshake, so an error answer would stop a hostname being served for as long as the condition lasted.

The cost of a lookup SHOULD grow with the number of stored certificates that could cover the hostname, not with the number stored.
A lookup for a hostname that no stored certificate could cover SHOULD be answered without parsing any certificate.

> [!NOTE]
> Retired certificates are kept, so the store grows without bound over the life of a host.

## Expiring certificates

While the certificate selected for a managed hostname is a `manual` or `csr` certificate whose `notAfter` is within fourteen days, the runtime MUST hold a fault of kind `cert_expiring_soon` against each ingress declaring the hostname.
The fault MUST name the hostname, the certificate's origin, its expiry and its identifier, and MUST direct the operator to [renew it](#certificate-signing-requests) or upload a replacement.
The runtime MUST clear the fault when the certificate selected for the hostname is no longer such a certificate, or when the ingress no longer exists.

Certificates of other origins are renewed by the runtime or by a strategy chain, and MUST NOT raise this fault.

## Hostname state

For each managed hostname, the runtime MUST make available to the [Operator Interface](../../overview.md#operator-interface):

- the apps declaring it;
- the policy that applies, and the pattern it matched;
- the selected or proxy-held certificate, with its origin, issuer, `notBefore` and `notAfter`;
- its acquisition status, per [ingress certificate observation](../observation.md#ingresses);
- the outcome and error of its latest issuance attempt;
- any retry block, and any recorded retry not yet consumed;
- the time of its next expected issuance, and whether that time comes from a suggested window, the one-third rule, the after-failure interval, or an issuance due now.

These values MUST be derived from the decision of [issuance](#issuance), so that the operator sees what the runtime will do.
The view MUST be available filtered to the hostnames one app declares.
A hostname declared only by an app whose status is `NotInstalled` MUST NOT appear.

The wire form of the view is specified in [TLSM](../../interface/tls.md).
