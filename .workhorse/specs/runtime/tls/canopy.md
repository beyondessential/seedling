---
id: TCAN
---

# Canopy certificate issuance

## Canopy

<!-- from runtime: TLS Certificate Strategy / Canopy -->

> r[tls.strategy.canopy]
> The Canopy strategy obtains a hostname's certificate from Canopy, through the [relay](rule:i/canopy.relay).
> Seedling holds the key and submits a signing request, and Canopy proves control of the name, issues, and renews.
> Canopy applies to a hostname when the name is one a public CA can issue for and at least one app declaring the hostname has a [Canopy application type](rule:l/app.canopy-type).
> It is available for such a hostname while an offer is live.
> The runtime does not consult Canopy's entitlements before asking.
> The request is how an operator comes to know that a hostname wants a certificate and authorises it, and declining to ask until an operator had set the name up first would make setting up a hostname a race over who configures it first.

> r[tls.canopy.key]
> The runtime generates the keypair for a Canopy certificate itself, stores the private key encrypted at rest using the [secret key](rule:r/secret.key), and sends Canopy only a signing request.
> A hostname's key is reused for every request and collection until Canopy reports that it must be replaced, so that asking again returns what Canopy already holds rather than opening a new order.
> A replaced key is destroyed once nothing the runtime serves depends on it.

> r[tls.canopy.request]
> One request is made per hostname, however many apps declare it.
> The signing request names exactly the hostname, so a wildcard policy results in a request for each concrete hostname it matches and never one for the wildcard.
> The request carries the Canopy application type when the typed apps declaring the hostname share exactly one, and omits it otherwise.

> r[tls.canopy.collect]
> Canopy answers a first request as pending and delivers the certificate when asked again.
> While a request is pending the runtime asks again on every Canopy report tick.
> Once a certificate is issued the runtime asks again, on every tick, from the certificate's renewal point until the certificate it holds has been replaced by a longer-lived one, which is how a renewal Canopy has carried out is collected.
> Between those moments nothing is asked.

> r[tls.canopy.outcomes]
> Canopy's answers are classified as follows:
>
> | Answer | Meaning |
> |---|---|
> | issued, usable | the certificate is stored and served |
> | pending | the order is open; asked again on the next tick |
> | refused, because the server lacks the TLS grant, the name is outside the domains its group controls, an operator has denied the name to it, it contradicts the application type named, or no zone Canopy manages covers the name | a refusal, per [tls.chain.refusal](rule:r/tls.chain.refusal) |
> | refused, because the server is paused or not attached to a live server, or because Canopy has no declaration of the name | not a refusal; retried |
> | order failed | not a refusal; retried, with Canopy's reason recorded |
> | the relay failed, or no offer is live | not a refusal; retried |
> | the signing request itself rejected as malformed | not a refusal; retried, and reported as a fault since the fault is the runtime's own |
>
> Only a cause Canopy itself names counts, so an answer from anything else on the way between the runtime and Canopy is retried rather than read as any of the above.

> r[tls.canopy.revoked]
> When Canopy reports a certificate revoked, or its key condemned, the certificate is no longer served and the runtime generates a new key and submits a new request.
> The hostname has no usable certificate from then, so it passes to the next strategy in its chain at once and stays there until the next cycle, rather than serving nothing while the new order runs.
> The runtime learns of a revocation from the certificates Canopy lists as held in the response to each report, without waiting for a collection.

> r[tls.canopy.serve]
> A Canopy certificate is stored with the origin `canopy` and served like any other runtime-managed certificate, per [tls.cert.serve](rule:r/tls.cert.serve), ranked against other certificates covering the hostname per [tls.strategy.manual](rule:r/tls.strategy.manual).
> A Canopy certificate counts as the active certificate for its hostname in [observe.ingress.certs](rule:r/observe.ingress.certs), so it satisfies [`rt.warm_certs`](rule:l/rt.warm-certs) barriers and an ingress's `Ready` state like any other runtime-managed certificate.
> A pending Canopy order is observed as `pending` and holds such a barrier as any acquisition in progress does.

> r[tls.canopy.attempts]
> Each Canopy order is one entry in the [attempt log](rule:r/tls.cert.attempt-log), opened by the request that creates it and closed when the certificate is collected, the order fails, or Canopy refuses.
> Asking again on a tick does not add an entry.

> r[tls.canopy.fault]
> The runtime files a fault of kind `canopy_certificate_failed` against each ingress declaring a hostname when a Canopy request was refused or failed and the hostname has no usable certificate, or when it has reached the last strategy in its chain without a replacement.
> The fault names the hostname and Canopy's reason, and is cleared when a usable certificate replaces the one in doubt.
> A hostname that holds a usable certificate and is still inside a strategy's window files nothing for Canopy trouble, since the chain is still working on it.

> r[tls.strategy.canopy-only]
> Operators may bind a hostname or wildcard pattern to the Canopy-only policy, which makes Canopy the chain's only strategy.
> Such a hostname never passes to HTTP-01 or the proxy's internal CA, so it is suited to a hostname whose traffic Seedling does not yet receive and for which a challenge could not succeed.
> A hostname under the policy follows [tls.chain.cycle](rule:r/tls.chain.cycle) and [tls.chain.no-certificate](rule:r/tls.chain.no-certificate): a valid certificate is left alone until its renewal point, and a failing or refused Canopy request is retried rather than abandoned, with no limit and with the fault of [tls.canopy.fault](rule:r/tls.canopy.fault) while it fails.
> Canopy is never skipped for want of a live offer, and applies as [tls.strategy.canopy](rule:r/tls.strategy.canopy) says.

> r[tls.canopy-only.untyped]
> When Canopy cannot be asked about a Canopy-only hostname, because no app declaring it has a Canopy type or because no public CA can issue for the name, the runtime files a fault of kind `canopy_certificate_failed` against each ingress declaring it, naming the hostname and saying which.
> The fault is cleared when a declaring app gains a type, when the policy is changed or cleared, or when no ingress declares the hostname.

> r[tls.policy.apply]
> Changes to operator-defined TLS policy (binding a hostname to a strategy, changing its parameters, or clearing it) must take effect on a subsequent reconciliation tick without operator-initiated apply steps.
> The runtime must rebuild the proxy configuration accordingly.
