---
id: CNR
---

# Canopy reporting

## Canopy Reporting

<!-- from runtime: Canopy Reporting -->

Seedling reports to Canopy so that a host and the apps it runs are visible in the fleet view, using the relay defined in the interface spec. It reports as a source distinct from any other agent on the host, so that its checks and the issues derived from them are scoped to it alone.

Each report describes the machine, with Seedling's own health, and each app that declares a [Canopy application type](rule:l/app.canopy-type), as an application of that type.

> r[canopy.settings.enabled]
> Whether Canopy access is enabled is stored durably and survives restarts.
> A Seedling instance that has never been configured has it enabled: on a host with no client offering Canopy, nothing is registered and nothing runs, so the default costs nothing.

> r[canopy.report.schedule]
> While Canopy access is enabled and at least one offer is live, the runtime reports its status every sixty seconds.
> When Canopy access is disabled, or no offer is live, no report is attempted.
> The absence of a provider is a deployment choice rather than a malfunction, and files no fault unless an app relies on Canopy (see [canopy.offer.absent](rule:r/canopy.offer.absent)).
> The runtime never initiates a connection of its own to obtain one.

> r[canopy.report.identity]
> Reports are addressed to a Canopy server identifier that the runtime resolves through the relay, by asking Canopy which server the offering client's identity is enrolled as.
> The identifier is cached durably so that the resolution is not repeated on every report, and is re-resolved after any report that fails in a way suggesting the cached value is wrong or absent.

> r[canopy.report.checks]
> Each report carries a fixed set of named checks on the machine, describing Seedling itself, so that the check catalog Canopy maintains does not grow with the set of applications an operator installs:
>
> | Check | Passed | Warning | Failed |
> |---|---|---|---|
> | `health/apps` | every registered app is running | at least one is degraded | at least one is faulted |
> | `health/faults` | no active faults | — | at least one active fault |
> | `health/proxy` | the ingress proxy is running | — | it is stopped |
> | `health/resolver` | the resolver is running | — | it is stopped |
>
> Apps in a transitional state — not installed, installing, deregistering, or running a lifecycle operation — do not make `health/apps` fail or warn.
> Each check carries the names of the apps or faults responsible for its result, so an operator reading the report can act on it without a second lookup.

> r[canopy.report.checks.undeterminable]
> A check the runtime cannot evaluate must be reported as broken rather than as failed.
> Reporting "the proxy is stopped" when the truth is "the runtime could not ask" would open an incident against a component that may be perfectly healthy, and a fleet-wide false alarm is worse than a missing signal.
> A broken result neither opens nor closes the check's issue, so a known failure keeps its standing while the inability to check is itself surfaced.

> r[canopy.report.extra]
> Each report also carries, as machine detail, the Seedling version, the daemon's own uptime in seconds, the total number of registered apps, a map of app status to count, the number of lifecycle operations in progress, and the number of active faults.
> It does not carry the hostname, the host's uptime, or the version of any managed application: those describe the host rather than Seedling, and are reported by the agent that owns them.

> r[canopy.report.fault]
> When a report fails while an offer is live, the runtime files a fault of kind `canopy_report_failed` carrying the failure detail.
> The fault is cleared automatically when a subsequent report succeeds, and when Canopy access is disabled or the last offer ends — in both of those cases reporting is no longer expected, so a lingering fault would misdescribe the instance.

> r[canopy.report.backup-prompt]
> A report's response carries instructions for the reporting source, including a list of backups to run immediately.
> That list is addressed only to the source that owns backups on a host, so a Seedling report never receives a non-empty one and the runtime does not act on it.
> The runtime must not treat an empty list as a signal of any kind.

> r[canopy.offer.absent]
> An app relies on Canopy while its definition declares a [Canopy application type](rule:l/app.canopy-type).
> While Canopy access is enabled and no offer has been live for fifteen minutes, the runtime holds an app-level fault of kind `canopy_offer_absent` against each app that relies on Canopy, stating when an offer was last live, or that none ever has been.
> The period runs from the latest of the last moment an offer was live, the moment the runtime started, the moment Canopy access was last enabled, and the moment the app came to rely on Canopy, so neither a host returning from an outage nor an operator re-enabling access files the fault before an offering client has had the period to connect.
> The last moment an offer was live is recorded durably, and remains accurate to within a reporting interval when the runtime stops without warning.
> The fault is cleared when an offer becomes live, when Canopy access is disabled, when the app stops relying on Canopy, or when the app is deregistered.
> It is held durably, and a restart does not clear it, since a restart is not one of those events.
> A short absence, such as the offering client restarting, files nothing; a host that has gone without an offer long enough to fall behind on what its apps need from Canopy files a fault on each app affected.
> Disabling Canopy access is an operator's decision, visible as such, and files nothing.

## Applications

<!-- from runtime: Canopy Reporting / Applications -->

> r[canopy.app.key]
> Each app whose definition declares a Canopy application type is reported as an application of that type, under its app name as the key, since the name is what identifies an app on the host.
> Several apps may declare the same type, and each is reported as an application of its own.
> Canopy identifies an application by its key and type together, so an app replaced by one of another name, or reported with a different type, is a different application to Canopy.
> It starts without the permissions an operator granted the previous one in Canopy, such as managing its DNS names and TLS certificates, until an operator grants them again.
> An app that stops being reported, because it was deregistered, replaced, or its definition no longer declares the type, reads to Canopy as unreachable, with its checks left as last reported, until an operator archives it.

> r[canopy.app.health]
> Each reported application carries one check, `health/app`, graded from the app's [status](rule:i/app.status):
>
> | Status | Result |
> |---|---|
> | `Running` | passed |
> | `Degraded` | warning |
> | `Faulted` | failed |
> | `Operating` | the result graded before the operation began, or skipped when there was none |
> | `NotInstalled`, `Installing`, `Deregistering` | skipped |
>
> A lifecycle operation neither raises nor recovers the application's health, since an upgrade in progress is neither a failure nor evidence of recovery.
> Canopy closes a check's issue when the check is skipped, so an operating app carries its earlier result instead, and that result is recorded durably so a restart during the operation does not change it.
> The check's detail carries the app's status and its active faults, each with its kind, the resource it is filed against where there is one, and its description, so an operator reading the application in Canopy can see why without a second lookup.
