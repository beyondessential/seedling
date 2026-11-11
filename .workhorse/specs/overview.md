---
id: SEED
---

# Seedling

Seedling runs containerised apps on a single host, each app defined by a script and kept running by a daemon that operators control remotely.

## Requirements notation

The key words MUST, MUST NOT, REQUIRED, SHALL, SHALL NOT, SHOULD, SHOULD NOT, RECOMMENDED, NOT RECOMMENDED, MAY and OPTIONAL in Seedling's specifications are to be interpreted as described in BCP 14 ([RFC 2119](https://www.rfc-editor.org/rfc/rfc2119), [RFC 8174](https://www.rfc-editor.org/rfc/rfc8174)) when, and only when, they appear in all capitals, as here.

## External documents

| document | what it covers |
| --- | --- |
| [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119) and [RFC 8174](https://www.rfc-editor.org/rfc/rfc8174) | the requirement keywords above |
| [Rhai](https://rhai.rs/book/) | the scripting language BSL is written in, under [BSL](language/overview.md) |
| [TOML](https://toml.io/en/) | the bundle metadata file and web interface configuration, under [BND](language/bundle.md), [WEB](web/serving.md) |
| [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html) | Seedling versions, under [BND](language/bundle.md), [STS](interface/status.md) |
| [Cargo version requirement syntax](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#version-requirement-syntax) | version requirements, under [BND](language/bundle.md) |
| [POSIX `crontab`](https://pubs.opengroup.org/onlinepubs/9799919799/utilities/crontab.html), Issue 8 | schedule expressions, under [ACN](language/action.md) |
| [IANA Time Zone Database](https://www.iana.org/time-zones) | time zone names, under [ACN](language/action.md), [CONST](language/constants.md) |
| [SipHash](https://www.aumasson.jp/siphash/siphash.pdf) | the hash of schedule `H` fields, under [ACN](language/action.md) |
| [zxcvbn](https://www.usenix.org/conference/usenixsecurity16/technical-sessions/presentation/wheeler) | password strength estimation, under [ACN](language/action.md) |
| [OCI Image Format Specification](https://github.com/opencontainers/image-spec) | container images and their configuration, under [CTR](language/container.md), [DEFN](interface/definitions.md), [IMGM](interface/images.md) |
| [OCI Distribution Specification](https://github.com/opencontainers/distribution-spec) | registries and fetching definitions, under [DEFN](interface/definitions.md), [IMGM](interface/images.md) |
| [capabilities(7)](https://man7.org/linux/man-pages/man7/capabilities.7.html) | container capabilities, under [CTR](language/container.md) |
| [signal(7)](https://man7.org/linux/man-pages/man7/signal.7.html) | stop signals, under [CTR](language/container.md) |
| [RFC 9000](https://www.rfc-editor.org/rfc/rfc9000) | QUIC, under [TRN](interface/transport.md), [WIRE](interface/wire.md), [SHL](interface/shells.md), [FWD](interface/port-forwards.md), [RLY](interface/canopy-relay.md), [ACTU](runtime/actuation.md) |
| [RFC 9001](https://www.rfc-editor.org/rfc/rfc9001) | TLS over QUIC, under [TRN](interface/transport.md) |
| [RFC 9221](https://www.rfc-editor.org/rfc/rfc9221) | QUIC datagrams, under [TRN](interface/transport.md), [WIRE](interface/wire.md), [FWD](interface/port-forwards.md) |
| [RFC 9846](https://www.rfc-editor.org/rfc/rfc9846) | TLS 1.3, under [TRN](interface/transport.md), [ING](language/ingress.md) |
| [RFC 9147](https://www.rfc-editor.org/rfc/rfc9147) | DTLS 1.3, under [ING](language/ingress.md) |
| [RFC 7250](https://www.rfc-editor.org/rfc/rfc7250) | raw public keys in TLS, under [TRN](interface/transport.md) |
| [RFC 7301](https://www.rfc-editor.org/rfc/rfc7301) | ALPN, under [TRN](interface/transport.md) |
| [RFC 6066](https://www.rfc-editor.org/rfc/rfc6066) | server name indication, under [TLS](runtime/tls/certificates.md) |
| [RFC 8032](https://www.rfc-editor.org/rfc/rfc8032) | Ed25519, under [KEY](interface/keys.md) |
| [RFC 8410](https://www.rfc-editor.org/rfc/rfc8410) | Ed25519 keys in PKIX, under [KEY](interface/keys.md), [TRN](interface/transport.md) |
| [RFC 5958](https://www.rfc-editor.org/rfc/rfc5958) | private key encoding, under [KEY](interface/keys.md), [TLSM](interface/tls.md) |
| [RFC 5280](https://www.rfc-editor.org/rfc/rfc5280) | X.509 certificates and SubjectPublicKeyInfo, under [TRN](interface/transport.md), [TLS](runtime/tls/certificates.md), [TLSM](interface/tls.md), [TCAN](runtime/tls/canopy.md) |
| [RFC 2986](https://www.rfc-editor.org/rfc/rfc2986) | certificate signing requests, under [TLS](runtime/tls/certificates.md), [TLSM](interface/tls.md), [TCAN](runtime/tls/canopy.md) |
| [RFC 7468](https://www.rfc-editor.org/rfc/rfc7468) | PEM encoding, under [TLS](runtime/tls/certificates.md), [TLSM](interface/tls.md) |
| [RFC 9525](https://www.rfc-editor.org/rfc/rfc9525) | service identity and wildcard matching, under [TLS](runtime/tls/certificates.md) |
| [RFC 8555](https://www.rfc-editor.org/rfc/rfc8555) | ACME, under [TLS](runtime/tls/certificates.md), [TLSM](interface/tls.md) |
| [RFC 9773](https://www.rfc-editor.org/rfc/rfc9773) | ACME renewal information, under [TLS](runtime/tls/certificates.md), [TLSM](interface/tls.md) |
| [ACME Profiles Extension](https://datatracker.ietf.org/doc/draft-ietf-acme-profiles/) (Internet-Draft) | certificate profiles, under [TLS](runtime/tls/certificates.md) |
| [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457) | problem details in Canopy answers, under [TCAN](runtime/tls/canopy.md) |
| [FIPS 180-4](https://csrc.nist.gov/pubs/fips/180-4/upd1/final) | SHA-256, under [TRN](interface/transport.md), [DEFN](interface/definitions.md), [INFRA](runtime/infrastructure.md), [WEB](web/serving.md) |
| [FIPS 186-5](https://csrc.nist.gov/pubs/fips/186-5/final) | ECDSA P-256, under [WEB](web/serving.md) |
| [RFC 9106](https://www.rfc-editor.org/rfc/rfc9106) | Argon2 password hashes, under [WEB](web/serving.md) |
| [PHC string format](https://github.com/P-H-C/phc-string-format/blob/master/phc-sf-spec.md) | password hash encoding, under [WEB](web/serving.md) |
| [RFC 4648](https://www.rfc-editor.org/rfc/rfc4648) | base64 and hex encodings, under [DEFN](interface/definitions.md), [TCAN](runtime/tls/canopy.md) |
| [RFC 1952](https://www.rfc-editor.org/rfc/rfc1952) | gzip, under [DEFN](interface/definitions.md) |
| [RFC 8259](https://www.rfc-editor.org/rfc/rfc8259) | JSON, under [WIRE](interface/wire.md), [AUD](runtime/audit-log.md), [WEB](web/serving.md) |
| [JSON Lines](https://jsonlines.org/) | the audit log format, under [AUD](runtime/audit-log.md) |
| [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) | timestamps, under [APPS](interface/apps.md), [EVT](interface/events.md), [FLTS](interface/faults.md), [FWD](interface/port-forwards.md), [IMGM](interface/images.md), [LOG](interface/logs.md), [RLY](interface/canopy-relay.md), [SHL](interface/shells.md), [TPL](interface/templates.md), [WEB](web/serving.md) |
| [RFC 9562](https://www.rfc-editor.org/rfc/rfc9562) | UUIDs, under [IDN](runtime/identity.md) |
| [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110) | HTTP semantics, under [HTTP](language/http-service.md), [ING](language/ingress.md), [PRX](runtime/proxy.md), [SITE](runtime/site-resources.md), [RLY](interface/canopy-relay.md), [WEB](web/serving.md), [ACTU](runtime/actuation.md) |
| [RFC 9111](https://www.rfc-editor.org/rfc/rfc9111) | HTTP caching, under [WEB](web/serving.md) |
| [RFC 9112](https://www.rfc-editor.org/rfc/rfc9112) | HTTP/1.1, under [ING](language/ingress.md), [PRX](runtime/proxy.md), [RLY](interface/canopy-relay.md) |
| [RFC 9113](https://www.rfc-editor.org/rfc/rfc9113) | HTTP/2, under [ING](language/ingress.md), [PRX](runtime/proxy.md) |
| [RFC 9114](https://www.rfc-editor.org/rfc/rfc9114) | HTTP/3, under [ING](language/ingress.md), [PRX](runtime/proxy.md) |
| [RFC 6585](https://www.rfc-editor.org/rfc/rfc6585) | status 429, under [PRX](runtime/proxy.md) |
| [RFC 8246](https://www.rfc-editor.org/rfc/rfc8246) | immutable responses, under [WEB](web/serving.md) |
| [RFC 3986](https://www.rfc-editor.org/rfc/rfc3986) | URIs, under [PRX](runtime/proxy.md) |
| [RFC 8878](https://www.rfc-editor.org/rfc/rfc8878) | Zstandard compression, under [HTTP](language/http-service.md) |
| [RFC 2047](https://www.rfc-editor.org/rfc/rfc2047) | encoded header words, under [WEB](web/serving.md) |
| [RFC 7034](https://www.rfc-editor.org/rfc/rfc7034) | `X-Frame-Options`, under [WEB](web/serving.md) |
| [Content Security Policy Level 3](https://www.w3.org/TR/CSP3/) | the console's content security policy, under [WEB](web/serving.md) |
| [Permissions Policy](https://www.w3.org/TR/permissions-policy/) | the console's permissions policy, under [WEB](web/serving.md) |
| [Fetch](https://fetch.spec.whatwg.org/) and [HTML](https://html.spec.whatwg.org/) | cross-origin isolation headers, under [WEB](web/serving.md) |
| [Secure Contexts](https://www.w3.org/TR/secure-contexts/) | loopback origins, under [WEB](web/serving.md) |
| [WebTransport](https://www.w3.org/TR/webtransport/) and [WebTransport over HTTP/3](https://datatracker.ietf.org/doc/draft-ietf-webtrans-http3/) (Internet-Draft) | the web interface's session transport, under [WEB](web/serving.md) |
| [Tailscale Serve identity headers](https://tailscale.com/kb/1312/serve) | Tailscale authentication, under [WEB](web/serving.md) |
| [RFC 1035](https://www.rfc-editor.org/rfc/rfc1035) and [RFC 1123](https://www.rfc-editor.org/rfc/rfc1123) | DNS names, under [ING](language/ingress.md), [DNS](runtime/resolver.md), [SITE](runtime/site-resources.md) |
| [RFC 3596](https://www.rfc-editor.org/rfc/rfc3596) | AAAA records, under [SITE](runtime/site-resources.md) |
| [RFC 4291](https://www.rfc-editor.org/rfc/rfc4291) | IPv6 addressing, under [NAT64](runtime/nat64.md) |
| [RFC 4193](https://www.rfc-editor.org/rfc/rfc4193) | unique local IPv6 addresses, under [INFRA](runtime/infrastructure.md) |
| [RFC 6146](https://www.rfc-editor.org/rfc/rfc6146) | stateful NAT64, under [NAT64](runtime/nat64.md), [CONST](language/constants.md), [SITE](runtime/site-resources.md) |
| [RFC 6147](https://www.rfc-editor.org/rfc/rfc6147) | DNS64, under [NAT64](runtime/nat64.md), [CONST](language/constants.md) |
| [RFC 6052](https://www.rfc-editor.org/rfc/rfc6052) | IPv4-embedded IPv6 addresses, under [NAT64](runtime/nat64.md), [SITE](runtime/site-resources.md) |
| [RFC 7050](https://www.rfc-editor.org/rfc/rfc7050) | NAT64 prefix discovery, under [NAT64](runtime/nat64.md) |

## Terminology

### Operator

A person or automated system that manages a host's apps through the [Operator Interface](#operator-interface).

### Host

The machine on which one Seedling daemon runs and manages apps.

### Runtime

The part of the Seedling daemon that evaluates app definitions, observes the host, and acts on it.

### Operator Interface

The network interface through which operators observe and control a host.
Defined in [OI](interface/overview.md).

### Web interface

The service that serves the console to browsers and relays their requests to a host.
Defined in [WEB](web/serving.md#web-interface).

### Console

The browser application through which an operator works with a host.
Defined in [UI](web/console.md#console).

### Canopy

The external fleet service a host reports its status to and obtains certificates from.
Defined in [RLY](interface/canopy-relay.md#canopy).

### Actor

The principal a request, and the events it causes, are attributed to.
Defined in [WIRE](interface/wire.md#actor).

### Fingerprint

The identifier of a key, derived from its public half.
Defined in [TRN](interface/transport.md#fingerprint).

### App

A named unit of software installed on a host, defined by one definition bundle.
Defined in [APP](language/app.md).

### Definition bundle

The tree of files, including one or more script files, that defines an app.
Defined in [BND](language/bundle.md).

### Script

The BSL source that defines an app, formed from its definition bundle's script files.
Defined in [BSL](language/overview.md#script).

### Resource

A single declared thing an app is made of, such as a deployment, service, ingress or volume.
Defined in [BSL](language/overview.md#resource).

### Resource graph

The set of resources produced by evaluating an app's script with its parameter values.
Defined in [BSL](language/overview.md#resource-graph).

### Static resource

A resource declared when an app's script is evaluated, outside any action.
Defined in [APP](language/app.md#static-resource).

### Action context

Evaluation inside a closure the runtime runs as part of a lifecycle operation.
Defined in [APP](language/app.md#action-context).

### Action

A named task an app declares, for an operator or the runtime to run.
Defined in [ACN](language/action.md#action).

### Action closure

The script function an action runs.
Defined in [ACN](language/action.md#action-closure).

### Runtime handle

The value through which an action closure directs the runtime.
Defined in [RT](language/rt.md#runtime-handle).

### Barrier

A wait in an action closure for resources to reach a lifecycle state.
Defined in [RT](language/rt.md#barrier).

### Ingress

An externally reachable endpoint of an app that delivers traffic to one of its services.
Defined in [ING](language/ingress.md#ingress).

### Volume

Persistent storage an app declares for its containers.
Defined in [VOL](language/volume.md#volume).

### External volume

A volume the host supplies to an app under a name the app declares.
Defined in [VOL](language/volume.md#external-volume).

### Site volume

A volume an operator manages independently of any app.
Defined in [SITE](runtime/site-resources.md#site-volume).

### Site ingress

A hostname an operator or provider manages independently of any app.
Defined in [SITE](runtime/site-resources.md#site-ingress).

### Generation

A number identifying one state of an app's definition and parameter values.
Defined in [GEN](runtime/generation.md).

### Generation history

The durable per-app record of every generation.
Defined in [GEN](runtime/generation.md#generation-history).

### Desired state

The resources that should exist for an app, each with its intended lifecycle state.
Defined in [RCN](runtime/reconciliation.md#desired-state).

### Lifecycle state

The stage a resource instance has reached between being wanted and being cleaned up.
Defined in [LCY](runtime/lifecycle.md).

### Install phase

Where an app stands in being installed: `NotInstalled`, `Installing`, `Installed` or `Uninstalling`.
An install moves an app through `Installing` to `Installed`, as [ACN](interface/actions.md#install) specifies, and an uninstall through `Uninstalling` back to `NotInstalled`, as [TRD](runtime/app-teardown.md) specifies.

### Lifecycle operation

A single run of an action closure directing the runtime through a change to an app.
Defined in [OPS](runtime/operations.md).

### World observation history

The timeline of facts the runtime has observed about each resource instance.
Defined in [HIST](runtime/history.md#world-observation-history).

### Autonomous operations log

The record of operations the runtime performed without direction from an action closure.
Defined in [HIST](runtime/history.md#autonomous-operations-log).

### Action execution log

The record of a lifecycle operation's progress through its action closure.
Defined in [HIST](runtime/history.md#action-execution-log).

### Data directory

The host directory holding the runtime's persistent state.
Defined in [ACTU](runtime/actuation.md#data-directory).

### Host prefix

The IPv6 unique local prefix within which the runtime assigns every address on a host.
Defined in [INFRA](runtime/infrastructure.md#host-prefix).

### Fault

A record of a condition on a host that needs an operator's attention.
Defined in [FLT](runtime/faults.md#fault).
