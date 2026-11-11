---
id: CHAIN
---

# TLS strategy chains

Applies to the order in which [strategies](certificates.md#strategy) are made responsible for a [managed hostname](certificates.md#managed-hostname)'s certificate, and when responsibility passes from one to the next.

## Terminology

### Strategy chain

The ordered list of strategies that may supply a hostname's certificate.

### Usable certificate

A certificate inside its validity window that has not been revoked.

### Cycle

One run of a hostname's strategy chain, from the moment its certificate needs replacing until a replacement arrives.

### Responsible strategy

The strategy in a hostname's chain that is working on the hostname's certificate during a cycle.

### Fall-through point

The moment responsibility passes from a strategy to the next one in its chain.

### Refusal

An answer from a strategy's issuing authority declining to issue for the hostname.

## Strategy identifiers

| identifier | strategy |
| --- | --- |
| `canopy` | Canopy, per [TCAN](canopy.md) |
| `acme_http01` | a public certificate the ingress proxy obtains through ACME HTTP-01 |
| `internal_ca` | a certificate from the ingress proxy's internal CA |

## Default chain

The [runtime](../../overview.md#runtime) MUST give the default chain to every managed hostname that no [policy](certificates.md#policy) matches, except one that takes a [Tailscale certificate](certificates.md#tailscale-certificates):

- for an [internal name](certificates.md#internal-name), `internal_ca` alone;
- for any other hostname that [Canopy applies to](canopy.md#applicability-and-availability), `canopy` then `acme_http01`;
- for any other hostname, `acme_http01` alone.

The runtime MUST NOT ask Canopy about an internal name.
A certificate from the internal CA MUST be obtained without contacting any external authority.

The [Canopy-only policy](canopy.md#canopy-only-policy) gives a hostname the chain `canopy` alone.
An ACME DNS-01 policy puts a hostname on no chain.
A hostname that takes a Tailscale certificate is on no chain, whatever policy matches it.

A stored certificate covering the hostname is not a step in a chain.
It serves for as long as [certificate selection](certificates.md#certificate-selection) picks it, and the chain runs when it needs replacing.

## Cycles

The runtime MUST start a cycle for a hostname when it has no usable certificate, and when the certificate serving it, of whatever origin, reaches its [renewal point](certificates.md#renewal-point).
The runtime MUST leave alone a hostname whose serving certificate is usable and has not reached its renewal point, whatever its chain would now resolve to.
A change in which strategies are available, such as an offer connecting, an app gaining a Canopy type or a policy being set, MUST NOT start a cycle.

> [!NOTE]
> Replacing a working certificate early spends the issuing authority's capacity for no gain, and leaves no fallback if the new source then fails.

Where the certificate being replaced is one the runtime cannot renew by itself, such as a `manual` upload, the runtime MUST keep serving that certificate until a replacement outranks it.

If the runtime cannot read the certificate the ingress proxy holds for a hostname, it MUST NOT treat the hostname as having no certificate, and MUST NOT start a cycle on that account.
An operator retry MUST start a cycle regardless.

The runtime MUST record durably, for a hostname in a cycle, any position the clock does not determine: a refusal, a revocation, and an open order.
After a restart, the runtime MUST resume each hostname at the position its record and the clock give.

## Fall-through

Within a cycle the chain's strategies are made responsible in order, starting with the first.
The responsible strategy MUST keep working on the hostname until it succeeds, refuses, or reaches its fall-through point.

A strategy's fall-through point is halfway between the moment it became responsible and the expiry of the certificate being replaced.
At that point the next strategy MUST become responsible.
The last strategy in a chain has no fall-through point.

> [!NOTE]
> With a renewal point two thirds of the way through a certificate's validity window, the first strategy falls through at five sixths of the window and the second at eleven twelfths.
> The rule is the same for every chain, so adding a strategy to a chain needs no new schedule.

Each strategy MUST become responsible at the moment its predecessor's fall-through point passed, not at the moment the runtime notices.

## Refusal handling

When a strategy refuses a hostname that has a certificate to replace, the next strategy MUST become responsible at once, without waiting for the fall-through point.

The runtime MUST treat as a refusal only an answer the strategy's issuing authority gave.
A failure, a timeout, an authority that could not be reached, and a strategy that could not be asked MUST NOT be treated as a refusal, and the responsible strategy MUST be tried again until its fall-through point.

## Hostnames with no certificate

A hostname has no certificate to replace when it has never had one, or when its last one has expired.
The runtime MUST NOT make such a hostname wait for a fall-through point.

A certificate revoked by its issuer, while its `notAfter` lies ahead, MUST still count as the certificate being replaced, and [Canopy revocation](canopy.md#revocation) applies.

For a hostname with no certificate to replace:

- a strategy that is available for it and refuses or fails MUST be asked again no sooner than the [after-failure interval](certificates.md#after-failure-interval), and the hostname MUST NOT pass to the next strategy;
- a strategy that is not available for it MUST be skipped at once, and the next strategy made responsible;
- a hostname with an order open with a strategy MUST stay with that strategy until the order resolves, even if the strategy stops being available meanwhile.

Canopy is not available for a hostname while no offer is live, except as the only strategy in its chain.

## Return to the first strategy

A hostname that passed to a later strategy MUST stay with it until the next cycle starts, and the first strategy MUST be responsible again then.
An operator retry of the hostname, per [retries](certificates.md#retry-blocks-and-retries), MUST start a cycle at once, from the first strategy.

## Proxy strategies

The ingress proxy MUST NOT order or renew a certificate through `acme_http01` for a hostname unless `acme_http01` is the responsible strategy, or no strategy precedes it in the hostname's chain.
The proxy's own renewal schedule therefore MUST NOT start a cycle, and the proxy MUST NOT give a hostname whose chain begins with `canopy` an HTTP-01 certificate while Canopy is its responsible strategy.

The ingress proxy MUST NOT order a certificate for a hostname on [no chain](#default-chain).
Such a hostname is served the runtime-managed certificate a [lookup](certificates.md#serving) answers with, once the runtime holds one.

The runtime MUST NOT take a certificate the proxy holds out of service to enforce this.
For a hostname whose proxy-held certificate needs holding back, the runtime MUST instead set the proxy's renewal to begin no earlier than the moment the strategies before `acme_http01` reach their fall-through points.

Where no fall-through point says when `acme_http01` becomes responsible, as for a hostname under the Canopy-only policy, the runtime MUST hold the proxy's renewal back until one ten-thousandth of the held certificate's validity window remains, and no further.

> [!NOTE]
> The proxy cannot be told never to renew, so the least window it accepts is the bound.

For a hostname the proxy holds no certificate for, the runtime MUST withhold the proxy's own ordering while a strategy before `acme_http01` is responsible, and at all times for a hostname on no chain.
If the runtime cannot decide whether to withhold, it MUST let the proxy order.

> [!NOTE]
> Letting the proxy order early costs one certificate ordered sooner than the chain would like; withholding wrongly leaves the hostname served nothing.

A hostname whose only covering certificate is staged to begin later MUST NOT have the proxy held back while it waits, since that certificate cannot be served before it begins.
