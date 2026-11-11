---
id: STS
---

# Host status

Applies to the methods that report whether the [server](overview.md#server) is reachable and the overall state of its [host](../overview.md#host).

## Liveness

`/server/ping` takes no parameters and MUST return `{}`.
The server MUST answer it without consulting the container engine or any app's state, and it MUST NOT fail except when the transport does.

## Server status

`/server/status` takes no parameters and returns an object with these members.

| member | type | meaning |
| --- | --- | --- |
| `version` | string | the running Seedling version, a [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html) version |
| `hostname` | string | the host's name |
| `uptime_secs` | integer | the number of whole seconds since the daemon started |
| `spki_fingerprint` | string | the [fingerprint](transport.md#fingerprint) of the server's key |
| `apps_total` | integer or null | the number of registered apps |
| `apps_by_status` | object or null | for each [app status](apps.md#app-status) at least one app holds, the number of apps holding it, keyed by status |
| `active_operations` | integer or null | the number of lifecycle operations in progress; a queued operation is not in progress |
| `active_faults` | integer or null | the number of active [faults](../overview.md#fault) on the host |
| `active_shells` | integer | the number of open shell sessions |
| `active_forwards` | integer | the number of active port forwards |

The server MUST NOT perform any expensive computation to answer `/server/status`, such as consulting the container engine.
The server MUST NOT fail `/server/status` except when the transport does.
A member the server cannot determine MUST be `null`, and MUST NOT be reported as zero or as an empty object.

## Infrastructure status

`/infra/status` takes no parameters and returns an object with these members.

| member | type | meaning |
| --- | --- | --- |
| `proxy` | string or null | `"running"` if at least one instance of the host's [reverse proxy](../runtime/proxy.md) is running, otherwise `"stopped"` |
| `resolver` | string or null | `"running"` if at least one instance of the host's [resolver](../runtime/resolver.md) is running, otherwise `"stopped"` |

A member whose state the server cannot determine MUST be `null`, and MUST NOT be reported as `"stopped"`.
