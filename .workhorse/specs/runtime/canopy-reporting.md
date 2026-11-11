---
id: CNR
---

# Canopy reporting

Applies to the status reports the [runtime](../overview.md#runtime) sends Canopy about the Seedling daemon on a [host](../overview.md#host) and the [apps](../overview.md#app) it runs.

## Terminology

### Report tick

One turn of the runtime's reporting schedule, which recurs whether or not a report is sent on it.

### Relying on Canopy

An app relies on Canopy while its current definition declares a [Canopy application type](../language/app.md#canopy-type).

## Canopy access

Whether Canopy access is enabled MUST be stored durably and survive restarts.
A host that has never been configured MUST have Canopy access enabled.
The runtime MUST record when access last went from disabled to enabled.

> [!NOTE]
> On a host with no client offering Canopy, nothing is registered and nothing is sent, so enabling access by default costs nothing.

The operator surface for the setting, and the offers through which requests reach Canopy, are specified in [RLY](../interface/canopy-relay.md).

## Report schedule

The runtime MUST run a report tick every 60 seconds for as long as it runs.
On each tick, while Canopy access is enabled and at least one [offer](../interface/canopy-relay.md#offers) is live, the runtime MUST send one report.
While access is disabled or no offer is live, the runtime MUST NOT attempt a report, and MUST NOT open a connection of its own to reach Canopy.

The absence of an offer is a deployment choice, and MUST NOT raise a fault except for apps relying on Canopy, as [missing offers](#missing-offers) specifies.

Every request to Canopy MUST travel through the [relay](../interface/canopy-relay.md#relay-streams).
Paths below are relative to the Canopy base URL the offering client resolves.

## Server identity

Reports are addressed to the Canopy server identifier the offering client's identity is enrolled as.
The runtime MUST resolve it with a `GET` to `/servers/self`, taking the `server_id` member of the answer.
The runtime MUST cache the identifier durably and use the cached value while it has one.
If a report fails in a way that suggests the cached identifier is wrong or absent, the runtime MUST discard the cached identifier, so that the next report resolves it again.

## Report payload

A report is a `POST` to `/status/<server_id>` with a JSON body:

| member | type | meaning |
| --- | --- | --- |
| `source` | string | `seedling` |
| `healthy` | boolean | false when any machine check's result is `failed` or `warning`, otherwise true |
| `machine` | object | `health`, an array of the [machine checks](#machine-checks), and `detail`, the [machine detail](#machine-detail) |
| `applications` | object | the [applications](#applications), keyed by app name |

The runtime reports as the source `seedling`, distinct from any other agent on the host, so that its checks and the issues Canopy derives from them belong to it alone.

Each check is an object with members `check`, its name, and `result`, one of `passed`, `warning`, `failed`, `skipped` or `broken`, followed by the detail members of that check.

If the runtime cannot read the apps to report, the report MUST fail, and MUST NOT be sent without them.

> [!NOTE]
> Canopy reads an application missing from a report as unreachable.

## Machine checks

The machine checks are a fixed set describing Seedling itself, so that Canopy's check catalogue does not grow with the apps an operator installs.

| check | `passed` | `warning` | `failed` | detail members |
| --- | --- | --- | --- | --- |
| `health/apps` | no app is `Degraded` or `Faulted` | an app is `Degraded`, none `Faulted` | an app is `Faulted` | `degraded` and `faulted`, arrays of app names; `total`, the number of apps |
| `health/faults` | no fault is active | | a fault is active | `count`, the number of active faults; `kinds`, their distinct kinds |
| `health/proxy` | the ingress proxy is running | | the ingress proxy is not running | |
| `health/resolver` | the resolver is running | | the resolver is not running | |

App statuses are those of [app status](../interface/apps.md#status-derivation).
An app that is `NotInstalled`, `Installing`, `Deregistering` or `Operating` MUST NOT affect the result of `health/apps`.

If the runtime cannot determine a check's result, it MUST report the check as `broken`, and MUST NOT report it as `failed`.
A `broken` check MUST carry a `reason` detail member saying why.

> [!NOTE]
> Canopy neither opens nor closes a check's issue on `broken`, so a known failure keeps its standing while the inability to check is itself visible.
> Reporting the proxy as stopped when the runtime could not ask would open an incident against a component that may be healthy, across every host at once.

## Machine detail

| member | type | meaning |
| --- | --- | --- |
| `seedlingVersion` | string | the running Seedling version |
| `seedlingUptimeSecs` | integer | seconds since the daemon started |
| `appsTotal` | integer | the number of registered apps |
| `appsByStatus` | object | the number of apps in each status, keyed by status name |
| `activeOperations` | integer | the number of [lifecycle operations](../overview.md#lifecycle-operation) in progress |
| `activeFaults` | integer | the number of active [faults](../overview.md#fault) |

The status names are `not_installed`, `installing`, `uninstalling`, `operating`, `running`, `degraded` and `faulted`, for the app statuses `NotInstalled`, `Installing`, `Deregistering`, `Operating`, `Running`, `Degraded` and `Faulted`.

The report MUST NOT carry the host's name, the host's uptime, or the version of any app.
Those describe the host, and the agent that owns them reports them.

## Applications

Each registered app relying on Canopy MUST be reported as an application of its declared type, keyed by its app name.
Several apps MAY declare the same type, and each MUST be reported as an application of its own.

| member | type | meaning |
| --- | --- | --- |
| `type` | string | the app's Canopy application type |
| `health` | array | the one check `health/app` |
| `detail` | object | empty |

> [!NOTE]
> Canopy identifies an application by its key and type together.
> An app replaced by one of another name, or reported with another type, is a new application to Canopy, and starts without the permissions an operator granted the old one, such as managing its DNS names and TLS certificates.
> An app no longer reported reads to Canopy as unreachable, with its checks as last reported, until an operator archives it.

## Application health

The runtime MUST grade an application's `health/app` check from the app's status:

| status | result |
| --- | --- |
| `Running` | `passed` |
| `Degraded` | `warning` |
| `Faulted` | `failed` |
| `Operating` | the result last graded before the operation began, or `skipped` if there is none |
| `NotInstalled`, `Installing`, `Deregistering` | `skipped` |

The runtime MUST record each `passed`, `warning` or `failed` result durably, so that a restart during an operation does not change the result the operation reports.
When an app becomes `NotInstalled`, the runtime MUST discard its recorded result.

> [!NOTE]
> A lifecycle operation is neither a failure nor evidence of recovery.
> Canopy closes a check's issue when the check is `skipped`, so an operating app carries its earlier result instead.

The check's detail members are `status`, the app's status name as in [machine detail](#machine-detail), and `faults`, the app's active faults.
Each fault is an object with members `kind`, `description`, and, where the fault is filed against a resource, `resource`, written `<type>/<name>`, or whichever of the two is recorded.

## Report response

The response to a report lists the certificates Canopy holds for the server, which the runtime MUST use as [Canopy revocation](tls/canopy.md#revocation) specifies.

The response also lists backups to run now.
That list is addressed to the agent that owns backups on a host, and the runtime MUST NOT act on it.

## Report failure fault

If a report fails while Canopy access is enabled and an offer is live, the runtime MUST file a fault of kind `canopy_report_failed` carrying the failure, attributed to the daemon rather than to an installed app, and MUST log the failure.
Successive failures MUST be one fault, carrying the latest failure.
The runtime MUST clear the fault when a report succeeds, and as soon as access is disabled or the last offer ends.

## Missing offers

While Canopy access is enabled and no offer has been live for 15 minutes, the runtime MUST hold an app-level fault of kind `canopy_offer_absent` against each app relying on Canopy, saying when an offer was last live, or that none ever has been.
The runtime MUST log each such fault when it files it.

The 15 minutes MUST run from the latest of the last moment an offer was live, the moment the runtime started, the moment Canopy access was last enabled, and the moment the app came to rely on Canopy.
The runtime MUST record durably the last moment an offer was live, and MUST keep that record accurate to within one report tick when the runtime stops without warning.

The runtime MUST clear the fault when an offer becomes live, when Canopy access is disabled, when the app stops relying on Canopy, or when the app is deregistered.
A restart MUST NOT clear the fault.

> [!NOTE]
> A short absence, such as the offering client restarting, files nothing.
> The fault marks a host that has gone long enough without an offer to fall behind on what its apps need from Canopy.
> Disabling access is an operator's visible decision, and files nothing.
