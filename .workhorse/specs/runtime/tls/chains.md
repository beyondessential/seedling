---
id: CHAIN
---

# TLS strategy chains

## Strategy chains

<!-- from runtime: TLS Certificate Strategy / Strategy chains -->

A strategy chain is the ordered list of strategies that may supply a hostname's certificate.
The default chain is defined by [tls.strategy.default](rule:r/tls.strategy.default).
An operator policy names a chain of one.

> r[tls.chain.cycle]
> A chain runs in cycles.
> A cycle starts when a hostname has no usable certificate, and when the certificate serving it, of whatever origin, reaches its renewal point: the start of the CA's suggested renewal window when it supplied one, otherwise the moment a third of the certificate's lifetime remains.
> A usable certificate is one inside its validity window that has not been revoked.
> A hostname holding a usable certificate that has not reached its renewal point is left alone, whatever its chain would now resolve to.
> A change in what is available — an offer connecting, an app gaining a Canopy type, a policy being set — does not start a cycle on its own, since replacing a working certificate early spends the issuing authority's capacity for no gain and leaves no fallback if the new source then fails.
> A stored certificate that the runtime cannot renew by itself, such as a manual upload, starts a cycle at its renewal point in the same way.
> It keeps serving until a replacement outranks it.

> r[tls.chain.ladder]
> Within a cycle the chain's strategies are tried in order, and the strategy currently responsible keeps working on the hostname until it succeeds, refuses, or reaches its fall-through point.
> The first strategy is responsible from the start of the cycle.
> A strategy's fall-through point is halfway between the moment it became responsible and the expiry of the certificate being replaced, and the next strategy becomes responsible at that point.
> With a renewal point two thirds of the way through a certificate's lifetime, the first strategy falls through at five sixths and the second at eleven twelfths.
> The last strategy in a chain has no fall-through point.
> The rule is the same for every chain, so that adding a strategy to one does not require a new schedule.

> r[tls.chain.refusal]
> A strategy that refuses a hostname that has a certificate to replace passes it to the next strategy at once, without waiting for its fall-through point.
> A hostname with no certificate to replace is the exception, covered by [tls.chain.no-certificate](rule:r/tls.chain.no-certificate).
> Only a refusal the strategy's authority has actually given counts.
> A failure, a timeout, an authority that could not be reached, and a strategy that could not be asked are none of them a refusal, and are retried until the fall-through point, since reading "could not determine" as "no" would move a hostname off a strategy that is merely briefly unavailable.

> r[tls.chain.no-certificate]
> A hostname with no certificate to replace, because it has never had one or because its last one has expired, does not wait for a fall-through point, because there is no expiry to measure one from.
> A certificate revoked by its issuer is not in this case: it is still the certificate being replaced, and [tls.canopy.revoked](rule:r/tls.canopy.revoked) applies.
> A strategy that is available for it and refuses or fails keeps being asked, at the cadence of the after-failure debounce, and the hostname does not pass to the next strategy.
> A strategy that is not available — for Canopy, no live offer — is skipped at once, and the next strategy becomes responsible.
> A hostname that has an open order with a strategy stays with it until the order resolves, so an offer disappearing while an order is pending does not abandon the order.

> r[tls.chain.return]
> A hostname that passed to a later strategy stays with it until the next cycle starts, and the first strategy is tried again then.
> An operator retry of the hostname, per [tls.cert.force-retry](rule:r/tls.cert.force-retry), starts a cycle at once.

> r[tls.chain.proxy-gate]
> The proxy must not order or renew a certificate through HTTP-01 for a hostname unless HTTP-01 is the strategy responsible for it, or the hostname's chain has no earlier strategy.
> The proxy's own renewal timing therefore cannot start a cycle, and a hostname whose first strategy is Canopy does not receive an HTTP-01 certificate ahead of Canopy.
> A certificate the proxy already holds is never taken out of service to enforce this.
> Where no clock says when HTTP-01 becomes responsible, as for a hostname under the Canopy-only policy, the proxy is held back to the last moments before the certificate it holds expires, and not beyond.
