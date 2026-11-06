# Issue certificates through Canopy

Scenarios for the default chain, Canopy issuance and the Canopy-only policy. Ticked cases are covered by automated tests; unticked cases are still owed.

## Chain decision

- [x] An app with no Canopy type leaves its hostname on HTTP-01 alone (verifies spec: tls.strategy.default)
- [x] A typed app puts Canopy first, and apps of different types on one hostname send no type (verifies spec: tls.strategy.canopy, tls.canopy.request)
- [x] A name no public CA can issue for is the internal CA alone and Canopy is never asked (verifies spec: tls.strategy.default)
- [x] A usable certificate before its renewal point is left alone whatever it would now resolve to, including when an offer connects (verifies spec: tls.chain.cycle)
- [x] A proxy-held certificate past its renewal point starts with Canopy, with the proxy's turn at five sixths (verifies spec: tls.chain.ladder)
- [x] Each strategy gets half the time left, for chains of one, two and three (verifies spec: tls.chain.ladder)
- [x] The ladder answer does not depend on when it is asked, and a suggested window moves the whole ladder (verifies spec: tls.chain.ladder)
- [x] A stored certificate outranks one the proxy holds as the one serving (verifies spec: tls.chain.cycle)
- [x] A hostname with nothing to replace skips Canopy at once with no live offer, and stays with Canopy otherwise (verifies spec: tls.chain.no-certificate)
- [x] An open order keeps a hostname with Canopy when the offer goes (verifies spec: tls.chain.no-certificate)
- [x] A recorded refusal moves a hostname with a certificate on at once, and never one without (verifies spec: tls.chain.refusal)
- [x] A failure, timeout or unreachable relay is never a refusal (verifies spec: tls.chain.refusal)
- [x] A row recorded for an earlier certificate is ignored, so the next cycle starts from the first strategy (verifies spec: tls.chain.return)
- [x] An operator retry starts a cycle at once and bypasses the refusal cadence (verifies spec: tls.cert.force-retry)
- [x] A refused hostname is asked again at the after-failure cadence, an open order on every tick (verifies spec: tls.chain.no-certificate)
- [x] A retry block pauses asking Canopy (verifies spec: tls.cert.retry-block)
- [x] A Canopy-only hostname never leaves Canopy and has no fall-through point (verifies spec: tls.strategy.canopy-only)

## Canopy engine

- [x] Every answer in the outcomes table classifies as the table says, including an unrecognised state (verifies spec: tls.canopy.outcomes)
- [x] The first request stores an encrypted key before asking and opens one attempt; asking again adds neither (verifies spec: tls.canopy.key, tls.canopy.attempts)
- [x] A collected chain is stored, served, and closes its attempt (verifies spec: tls.canopy.collect, tls.canopy.serve)
- [x] A renewal replaces the chain under the same key only once a longer-lived one lands (verifies spec: tls.canopy.collect)
- [x] A chain for some other key is not stored (verifies spec: tls.canopy.key)
- [x] A refusal is logged as a failed attempt; an unreachable relay leaves no attempt (verifies spec: tls.canopy.attempts)
- [x] A revocation replaces the key, destroys the old one, opens a new order and passes the hostname on at once (verifies spec: tls.canopy.revoked)
- [x] A revocation learnt from a report matches on name and key fingerprint (verifies spec: tls.canopy.revoked)
- [x] A restart between the request and the collection finds the same key and position (verifies spec: tls.canopy.key)
- [ ] Two apps declaring one hostname result in one request
- [ ] A wildcard policy results in one request per matching concrete hostname, never for the wildcard

## Proxy gate

- [x] The endpoint withholds from a hostname the proxy holds nothing for while Canopy is responsible (verifies spec: tls.chain.proxy-gate)
- [x] It never withholds from a hostname the proxy holds a certificate for, could not be read, or was not looked at (verifies spec: tls.chain.proxy-gate)
- [x] It falls through once the proxy is responsible, and when the facts cannot be gathered (verifies spec: tls.chain.proxy-gate)
- [x] The proxy's renewal ratio is the fraction left when Canopy falls through, and a Canopy-only hostname holds it to the last moment (verifies spec: tls.chain.proxy-gate)
- [x] Hostnames held back get their own automation policy per ratio (verifies spec: tls.chain.proxy-gate)
- [ ] Against a real Caddy: a held certificate keeps serving while Canopy is responsible
- [ ] Against the production proxy image: renewal waits for the ratio, including where the CA supplies renewal information
- [ ] An idle hostname with no traffic that falls through to HTTP-01 gets a certificate

## Observation and faults

- [x] A stored certificate in its window satisfies a warm-cert check; expired, revoked and pending ones do not (verifies spec: tls.canopy.serve)
- [x] An open order is seen as one, so the acquisition deadline does not fault it (verifies spec: tls.canopy.serve)
- [x] A Canopy-only hostname nothing can ask about is faulted and cleared when the policy goes (verifies spec: tls.canopy-only.untyped)
- [x] A failing request with no usable certificate is faulted and cleared on arrival; trouble with a usable certificate files nothing (verifies spec: tls.canopy.fault)
- [x] A chain that ran out of strategies without a replacement is faulted (verifies spec: tls.canopy.fault)
- [ ] The expiring-certificate sweep never faults a Canopy certificate (verifies spec: tls.fault.expiring)

## Interface, CLI and web

- [x] The Canopy-only policy is set, listed, replaced and cleared, and refuses a pattern that can never match (verifies spec: tls.policy.set-canopy)
- [x] The hostname view carries the chain, the Canopy origin and last issuance (verifies spec: tls.hostname.list)
- [x] A retry for a hostname Canopy is not part of does not linger (verifies spec: tls.cert.retry)
- [x] The web page binds Canopy-only with or without a DNS provider, lists it, and shows the responsible strategy and its fall-through (verifies spec: routes.certificates)
- [ ] The CLI `tls policies set-canopy` round trip against a running daemon

## Migration

- [x] The upgrade keeps policies, certificates and which certificate each attempt produced, and accepts the new policy, origin and state (verifies spec: tls.strategy.canopy)
