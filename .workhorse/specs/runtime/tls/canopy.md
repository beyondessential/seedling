---
id: TCAN
---

# Canopy certificates

Applies to the Canopy [strategy](certificates.md#strategy), by which the [runtime](../../overview.md#runtime) obtains a [managed hostname](certificates.md#managed-hostname)'s certificate from Canopy, and to the Canopy-only policy.

## Borrowed terms

| term | meaning |
| --- | --- |
| certificate signing request (CSR) | a PKCS #10 certification request, per [RFC 2986](https://www.rfc-editor.org/rfc/rfc2986) |
| subject public key info (SPKI) | the public key structure of [RFC 5280](https://www.rfc-editor.org/rfc/rfc5280) §4.1.2.7 |
| problem details | the HTTP error body of [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457) |
| base64 | the encoding of [RFC 4648](https://www.rfc-editor.org/rfc/rfc4648) §4 |

## Terminology

### Key fingerprint

The lowercase hexadecimal SHA-256 digest of a key's DER-encoded SPKI, by which Canopy names a key.

## Applicability and availability

Canopy applies to a hostname when the hostname is not an [internal name](certificates.md#internal-name) and at least one app declaring it has a [Canopy application type](rule:l/app.canopy-type).
Canopy is available for a hostname it applies to while an [offer](rule:i/canopy.offer) is live.

Seedling holds the key and submits a CSR; Canopy proves control of the name, issues, and renews.
Every request to Canopy MUST travel through the [relay](rule:i/canopy.relay), and the runtime MUST NOT open a connection to Canopy of its own.

The runtime MUST NOT withhold a request on the strength of what Canopy reports the server is entitled to.

> [!NOTE]
> A request is how an operator in Canopy learns that a hostname wants a certificate and comes to authorise it.
> Waiting for the operator to set the name up first would make setting up a hostname a race over who configures it first.

## Keys

The runtime MUST generate the key pair for a Canopy certificate itself, and SHOULD generate an ECDSA key on curve P-256.
It MUST store the private key encrypted at rest with the [secret key](rule:r/secret.key) before the first request that uses it, and MUST send Canopy only a CSR.

The runtime MUST reuse a hostname's key for every request until Canopy reports the key condemned or its certificate revoked.
Asking again with the same key returns the order Canopy already holds, and opens no new one.

## Requests

The runtime MUST make at most one request per hostname at a time, however many apps declare it.
A request is a `POST` to `/certificates/request` with a JSON body:

| member | type | required | meaning |
| --- | --- | --- | --- |
| `name` | string | yes | the hostname |
| `csr` | string | yes | the CSR, DER, base64 |
| `application_type` | string | no | the Canopy application type |

The CSR's SAN set MUST name exactly the hostname.
A wildcard policy MUST result in a request for each concrete hostname it matches, and never in a request for the wildcard.
The request MUST carry `application_type` when the typed apps declaring the hostname share exactly one type, and MUST omit it otherwise.

The runtime asks Canopy about certificates only on a [report tick](../canopy-reporting.md#report-schedule) whose report succeeded.
The runtime SHOULD make its requests one at a time, SHOULD make no more than 8 in one tick, and SHOULD start none once 20 seconds of the tick have passed.
A tick that stops early SHOULD resume, on the next tick, after the last hostname it asked about.

## Collection

Canopy answers a first request as pending, and delivers the certificate when asked again.

While an order is open for a hostname, the runtime MUST ask again on every report tick, whichever strategy is responsible.
Once a Canopy certificate serving a hostname reaches its [renewal point](certificates.md#renewal-point), the runtime MUST ask again on every report tick until it has stored a certificate expiring later than the one it holds.
Asking from the renewal point is how the runtime collects a renewal Canopy has carried out.
At any other time the runtime MUST NOT ask.

After a refusal in the current [cycle](chains.md#cycle), the runtime MUST NOT ask to collect a renewal until the next cycle.
After a refusal, a failed order or a rejected CSR, the runtime MUST NOT ask again for that hostname until the [after-failure interval](certificates.md#after-failure-interval) has passed, unless an operator retries it.

## Answers

The runtime MUST classify each answer to a request as follows.

An answer Canopy gave with a success status:

| answer | classification |
| --- | --- |
| `key_must_be_replaced` true | [revoked](#revocation) |
| `revoked` true | revoked |
| `usable` true, with a `chain` | issued |
| `state` `pending` | pending |
| `state` `failed` | order failed, with `last_error` as the reason |
| `state` `revoked` | revoked |
| anything else | unavailable |

An answer with an error status, by its status and the problem details `type` member, of the form `/errors/<cause>`:

| status | cause | classification |
| --- | --- | --- |
| 403 | `auth-insufficient-permissions`, `name-not-entitled`, `dns-name-denied` | [refusal](chains.md#refusal) |
| 409 | `conflict`, `dns-name-type-mismatch` | refusal |
| 409 | `certificate-key-compromised` | revoked |
| 400 | `bad-request` | CSR rejected |
| any other | any other, or none | unavailable |

A relay failure, and the absence of a live offer, MUST be classified as unavailable.

The runtime MUST act on each classification:

- issued: store the certificate, as [accepting a certificate](#accepting-a-certificate) specifies;
- pending: record the order as open;
- refusal: record the refusal and its reason, and apply it to the chain as [refusal handling](chains.md#refusal-handling) specifies when Canopy is the responsible strategy;
- order failed, or CSR rejected: record the reason, and ask again no sooner than the after-failure interval;
- unavailable: record the reason, and ask again on the next report tick;
- revoked: proceed as [revocation](#revocation) specifies.

None of order failed, CSR rejected and unavailable is a refusal.
If the runtime fails to put a request at all, for a cause of its own, it MUST record the reason and ask again no sooner than the after-failure interval.

> [!NOTE]
> Only a cause Canopy names in its own form counts, so an error from anything between the runtime and Canopy is retried rather than read as a refusal, which would move the hostname off Canopy for the rest of the cycle.

Before recording a reason Canopy gave, the runtime MUST replace each control character in it with a space, and MUST keep at most its first 500 characters.

## Accepting a certificate

The runtime MUST store a chain Canopy issued only if the leaf's SAN set is exactly the hostname, the leaf is inside its validity window, and the leaf's public key is the key the runtime holds for the hostname.
A chain that fails any of these checks MUST be classified as unavailable, with the failed check as the reason.

While a Canopy certificate for the hostname is active, the runtime MUST store an issued chain only if both expiries are recorded and the new one is later; otherwise the answer MUST be treated as pending.

A stored Canopy certificate has origin `canopy`.
The runtime MUST store it, and retire what it [supersedes](certificates.md#supersession), in one transaction, and MUST record the end of the cycle in the same transaction.

## Revocation

The runtime learns of a revocation from an answer to a request, and from the certificates Canopy lists as held for the server in the response to each report.
A listed certificate MUST be matched to a stored Canopy certificate by its name, ignoring ASCII case, and by its key fingerprint.
A listed certificate with `revoked` or `key_must_be_replaced` true condemns the stored certificate it matches.

When a Canopy certificate is condemned, the runtime MUST stop serving it at once and MUST destroy its key, whether or not the tick has room for further requests.
If no cycle is running for the hostname, one MUST start, with the condemned certificate as the one being replaced.
The hostname MUST pass at once to the next strategy in its chain and stay there until the next cycle.
The runtime MUST generate a new key, and MUST request a certificate for it, on this tick if the tick has room and otherwise on a later one, and MUST go on asking for it until Canopy delivers or refuses.

## Serving and observation

A Canopy certificate MUST be served like any other runtime-managed certificate, per [serving](certificates.md#serving), and ranked against other certificates covering the hostname per [certificate selection](certificates.md#certificate-selection).

A Canopy certificate MUST count as the hostname's certificate for [ingress certificate observation](rule:r/observe.ingress.certs), so that it satisfies [`rt.warm_certs`](rule:l/rt.warm-certs) barriers and an ingress's `Ready` state.
An open Canopy order MUST be observed as `pending`.

## Attempt log entries

Each Canopy order MUST be one entry in the [attempt log](certificates.md#attempt-log), opened by the request that creates it and closed when the certificate is stored, the order fails, or Canopy refuses.
Asking again about an open order MUST NOT add an entry.

## Faults

The runtime MUST hold a fault of kind `canopy_certificate_failed` against each ingress declaring a hostname while any of these holds:

- Canopy is the responsible strategy for the hostname and cannot be asked about it;
- the hostname's chain includes Canopy, a cycle is running, no usable certificate serves the hostname, and a reason is recorded for the responsible strategy's last failure;
- a cycle replacing a certificate has reached the last strategy of a chain of more than one.

The fault MUST name the hostname and the reason.
The runtime MUST clear the fault when none of these holds, or when the ingress no longer exists.
The runtime MUST bring these faults into line on every report tick, whether or not the report succeeded, and MUST log each fault it files.

A hostname that holds a usable certificate and whose responsible strategy has not reached its fall-through point MUST NOT raise the fault, since its chain is still working on it.

## Canopy-only policy

An operator MAY bind a hostname or hostname pattern to the Canopy-only policy, which makes Canopy the only strategy in the hostname's chain.
Its identifier in policy listings is `canopy`.
A hostname under the policy MUST NOT pass to HTTP-01 or the internal CA.

> [!NOTE]
> The policy suits a hostname whose traffic does not yet reach the host, for which a challenge could not succeed.

A hostname under the policy follows [cycles](chains.md#cycles) and [hostnames with no certificate](chains.md#hostnames-with-no-certificate): a usable certificate is left alone until its renewal point, and a failing or refused request is retried without limit, with the fault of [faults](#faults) held while it fails.
Canopy MUST NOT be skipped for such a hostname for want of a live offer.

When Canopy cannot be asked about a hostname under the policy, because no app declaring it has a Canopy type or because it is an internal name, the `canopy_certificate_failed` fault MUST say which.
It clears when a declaring app gains a type, when the policy is changed or cleared, or when no ingress declares the hostname.
