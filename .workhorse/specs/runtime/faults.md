---
id: FLT
---

# Faults

Applies to how the [runtime](../overview.md#runtime) records, files and clears faults, and to every fault kind it files.

## Terminology

### Fault

A record that one thing on a [host](../overview.md#host) is faulty in a named way and needs an [operator](../overview.md#operator)'s attention.

### Fault kind

The name of one way a thing can be faulty, such as `crash_loop`.

### Subject

The thing a fault is about: an instance, a resource, an image reference, a hostname, or nothing more specific than the app.

### Condition fault

A fault that is active exactly while a condition on the host holds.

### Event fault

A fault recording that something happened, active from that occurrence until a named clearing event.

### Latched fault

A fault deliberately kept active after its trigger has passed, until a named lifecycle event.

## Fault records

A fault is identified by its app, its kind and its subject.
At most one fault MUST be active for a given app, kind and subject.
Filing a fault whose key is already active MUST NOT create a second fault.

A fault's subject MUST be the thing that is faulty, never a part of the fault's description.
A fault about an [app](../overview.md#app) as a whole has an empty subject.

The runtime MUST file a fault about the host rather than an app against the app name `_system`.
The runtime MUST file a fault about its own reporting to other systems against the app name `seedling`.

The runtime MUST NOT pass a fault to a BSL script to handle.

The runtime MUST surface every active fault through the [Operator Interface](../overview.md#operator-interface), in the [fault record](../interface/faults.md#fault-records) shape.
The runtime MUST emit an [event](../interface/events.md) when it files a fault and when it clears one.
The runtime SHOULD log an error when it files a fault.

## Filing and clearing

Every fault kind MUST state when it is filed, when it clears, and its shape: condition, event or latched.

The runtime MUST clear a fault no more broadly than it files it.
An occurrence affecting one subject MUST NOT clear a fault held by another.
Within one evaluation, the faults the runtime files and the faults it clears MUST NOT share a key.

The runtime MUST determine whether a condition fault holds from durable state and from observations, never from state held only in memory, so that a condition fault is active exactly while its condition holds, across runtime restarts.
If the runtime cannot determine whether a fault's condition holds, it MUST neither file nor clear that fault.

Every active fault MUST be able to clear without a runtime restart, through its clearing condition or through an operator clearing it.
An operator MAY clear active faults through the Operator Interface, including faults filed against `_system` or `seedling`.
A condition fault an operator clears is filed again while its condition still holds.

When an app is deregistered, the runtime MUST clear every fault filed against it.
When a resource instance is removed, the runtime MUST clear every fault whose subject is that instance.

A fault on one resource MUST NOT prevent the runtime from managing other resources.

## Workload faults

The runtime MUST file and clear the faults below against the app the instance belongs to.

| kind | subject | filed when | clears when | shape |
| --- | --- | --- | --- | --- |
| `image_pull_failed` | the image reference | an instance needs an image whose pull has failed 5 consecutive times, as [IMG](images.md#image-pulls) specifies | the image is present on the host | condition |
| `container_start_failed` | the instance | the instance's supervised unit is observed failed while the instance's desired state is running | the unit is observed active or starting | condition |
| `start_failed` | the instance | starting the instance fails for a reason no other kind names | the instance is observed running | event |
| `stop_failed` | the instance | stopping the instance fails | a later stop of the instance succeeds, or the instance is observed running | event |
| `observe_failed` | the instance | observing the instance fails on 3 consecutive iterations of the reconciliation loop | an observation of the instance succeeds | condition |
| `registry_error` | the instance | the runtime cannot read the instance's recorded identity when starting it | the instance starts | event |
| `external_volume_not_mapped` | the instance | the instance mounts an external volume that has no mapping | the instance starts | condition |
| `crash_loop` | the instance | the instance's restart rate reaches the [configured threshold](autonomous.md#crash-loop-detection), or its supervisor has [given up restarting it](autonomous.md#restart-pacing) | the instance is observed healthy, or an operator clears it | latched |
| `health_check_failed` | the instance | the instance has a [healthcheck](../language/deployment.md#health-checks) and has been unhealthy without a break for longer than `start_period` plus `retries` × `interval` | the instance is observed healthy, or is unscheduled | condition |
| `health_check_replace_failed` | the Deployment | the [replacement guard](autonomous.md#replacement-guard) trips for the Deployment | the app's [generation](../overview.md#generation) changes, or an operator clears it | latched |

An `image_pull_failed` fault MUST name the image reference.
An `external_volume_not_mapped` fault MUST name the external volume, and the runtime MUST NOT start the instance while the mapping is missing.

A `crash_loop` fault MUST state which of its two triggers filed it.

A `health_check_replace_failed` fault MUST identify the failed replacement and the unhealthy original.

### Withholding faults

A withholding fault is one under which the runtime stops acting on its subject until the fault clears: `external_volume_not_mapped`, `crash_loop` ([no autonomous restart](autonomous.md#crash-loop-detection)) and `health_check_replace_failed` ([no further replacements](autonomous.md#replacement-guard)).
An app holding one is [`faulted`](../interface/apps.md#status-derivation) rather than `degraded`.

## Volume faults

The runtime MUST file and clear the faults below against the app the volume belongs to.

| kind | subject | filed when | clears when | shape |
| --- | --- | --- | --- | --- |
| `volume_create_failed` | the volume instance | creating the volume's storage fails | the volume's storage is created or observed present | event |
| `volume_remove_failed` | the volume instance | removing or holding the volume fails | a later removal of the volume succeeds | event |
| `observe_failed` | the volume instance | observing the volume fails on 3 consecutive iterations of the reconciliation loop | an observation of the volume succeeds | condition |

## App faults

The runtime MUST file and clear the faults below against the app, with an empty subject except where the table names one.

| kind | subject | filed when | clears when | shape |
| --- | --- | --- | --- | --- |
| `script_error` | none | the app's definition fails to evaluate, at registration, on an update, on a parameter change, on a change to the registry allowlist, when an undetermined [host constant](../language/constants.md#determining-host-constants) is determined, or when the runtime starts | the definition next evaluates | condition |
| `disallowed_registry` | none | an image a Deployment or Job declares is from a registry outside the [allowlist](../language/container.md#registry-allowlist) | an evaluation finds no declared image outside the allowlist | condition |
| `registry_error` | none | the runtime cannot compute the app's desired state | the runtime next computes it | condition |
| `operation_failed` | none | a [lifecycle operation](../overview.md#lifecycle-operation) for the app fails, or its action closure does not compile | a later lifecycle operation for the app completes | event |
| `operation_cancelled` | none | a lifecycle operation for the app is cancelled | a later lifecycle operation for the app completes | event |
| `install_interrupted` | none | at startup, an interrupted install cannot be resumed because its parameter values are missing or cannot be decrypted | a later install of the app completes, or an operator clears it | latched |
| `uninstall_unidentifiable_units` | none | an uninstall finds supervised units that may belong to the app but no recorded instance identities to match them against | the app's teardown completes | latched |
| `definition_unsupported` | none | the running Seedling does not satisfy the app's [Seedling version requirement](../language/bundle.md#seedling-version-requirement) | the definition is replaced by one the running Seedling satisfies, or the runtime starts as a version that satisfies it | condition |
| `definition_source_moved` | the digest the reference now selects | a successful [re-check](definition-recheck.md#resolution) finds that the reference in the app's fetched definition selects a digest other than the running one | a successful re-check selects the running digest again, or the app's definition is replaced | condition |
| `definition_recheck_failed` | none | 5 consecutive [re-checks](definition-recheck.md#back-off) of the app's definition fail | a re-check succeeds, or the app's definition is replaced | condition |
| `schedule_params_invalid` | the action | a schedule of the action comes due and its run [fails param validation](../language/action.md#scheduled-actions) | a later scheduled run of the action passes validation, or the definition no longer schedules the action | event |
| `canopy_offer_absent` | none | the app [relies on Canopy](canopy-reporting.md#missing-offers) and no offer has been live for 15 minutes | an offer is live, Canopy access is disabled, or the app stops relying on Canopy | condition |

An `operation_failed` fault MUST carry the failure.
A `disallowed_registry` fault MUST name each disallowed registry.
A `canopy_offer_absent` fault MUST say when an offer was last live, or that none ever has been.

A `definition_source_moved` fault MUST identify the reference and both digests.
A re-check that fails MUST neither file nor clear it.

A `definition_unsupported` fault MUST identify the requirement and the running version.
The runtime MUST evaluate and maintain the app's definition as usual while the fault is active.

## Ingress and service faults

The runtime MUST file and clear the faults below against the app owning the ingress or service, or against `_system` for a site ingress or site service.

| kind | subject | filed when | clears when | shape |
| --- | --- | --- | --- | --- |
| `ingress_conflict` | the app ingress or the site ingress | an app ingress and a site ingress attachment claim the same hostname and port, as [SITE](site-resources.md#ingress-conflicts) specifies, or an app ingress is left out because another app's ingress [claims](../language/ingress.md#claims-across-apps) them | the hostname and port are no longer in conflict | condition |
| `proxy_apply_failed` | the app ingress or the site ingress | applying a proxy configuration that includes the ingress fails | a later proxy configuration is applied | condition |
| `site_ingress_target_missing` | the site ingress attachment | a site ingress attachment forwards to an app service that does not exist | the attachment's target exists, or the attachment is removed | condition |
| `service_degraded` | the service | the service's routing pool has [fallen back](lifecycle.md#routing-pool) to backends that are running but unhealthy | a backend in the pool is healthy, or the service is unscheduled | condition |
| `site_service_endpoint_unresolvable` | the site service | an endpoint of the site service is [unresolvable](site-resources.md#endpoint-addressing) | no endpoint of the site service is unresolvable | condition |
| `site_service_endpoint_unroutable` | the site service | an endpoint of the site service is [unroutable](site-resources.md#endpoint-addressing) | no endpoint of the site service is unroutable | condition |

An `ingress_conflict` fault MUST identify the other party and the hostname and port.
A `proxy_apply_failed` fault MUST carry the most recent apply error.
A withheld apply, as [RCN](reconciliation.md#applying-absolute-state) specifies, MUST neither file nor clear `proxy_apply_failed`.
A site service fault MUST name the failing endpoints, and an unroutable one MUST state whether each needs NAT64 or IPv6 egress.

## Certificate faults

| kind | app | subject | filed when | clears when | shape |
| --- | --- | --- | --- | --- | --- |
| `cert_acquisition_failed` | the ingress's app | the ingress instance | no valid certificate has been observed for the ingress's hostname 180 seconds after it began to need one, while no acquisition is in progress | a valid certificate is observed for the hostname | condition |
| `cert_issue_failed` | `_system` | the hostname | issuing a certificate for the hostname through ACME with DNS validation or through Tailscale fails, or ACME issuance needs a contact email and none is set | a certificate for the hostname is issued, or no ingress needs one for it | event |
| `cert_expiring_soon` | the ingress's app | the ingress | a certificate the runtime cannot renew itself, [as TLS specifies](tls/certificates.md#expiring-certificates), expires within 14 days | the certificate selected for the hostname is no longer one the runtime cannot renew and expiring within 14 days, or the ingress no longer exists | condition |
| `canopy_certificate_failed` | the ingress's app | the ingress | the ingress's certificate cannot be obtained or replaced through Canopy, [as TCAN specifies](tls/canopy.md#faults) | none of the [conditions TCAN names](tls/canopy.md#faults) holds, or the ingress no longer exists | condition |

A `cert_acquisition_failed` fault MUST identify the hostname and the most recent acquisition error.
A `cert_issue_failed` fault MUST carry the failure.
If the runtime cannot read whether a certificate is held for a hostname, it MUST neither file nor clear `cert_acquisition_failed` for that hostname.

## Host faults

The runtime MUST file and clear the faults below against `_system`, with an empty subject.

| kind | filed when | clears when | shape |
| --- | --- | --- | --- |
| `container_runtime_unavailable` | the runtime is running without a working container runtime or process supervisor | the runtime has started with both reachable | condition |
| `app_priorities_unreadable` | the runtime cannot read the stored app priorities | the runtime next reads them | condition |
| `scaling_decisions_unreadable` | the runtime cannot read the stored operator scaling decisions | the runtime next reads them | condition |
| `slice_sync_failed` | applying the [resource-control groups](priority.md#resource-control-groups) has failed 5 consecutive times | the groups are next applied | condition |
| `nat64_setup_failed` | setting up the NAT64 translator fails | the translator is next set up | condition |
| `nat64_detection_failed` | [detecting](nat64.md#detection) existing NAT64 is undetermined | detection succeeds | condition |
| `nat64_teardown_failed` | tearing down the NAT64 translator fails | a teardown succeeds, or the translator is next set up | condition |
| `ipv6_egress_undetermined` | whether the host has [IPv6 egress](nat64.md#ipv6-egress) cannot be determined | the determination succeeds | condition |
| `ipv4_egress_undetermined` | whether the host has [IPv4 egress](../language/constants.md#host-constants) cannot be determined | the determination succeeds | condition |
| `timezone_undetermined` | the host's [local time zone](../language/constants.md#host-constants) cannot be determined | the determination succeeds | condition |
| `operation_state_unreadable` | the runtime cannot read the [persisted operation, its cancel, or the persisted queue](operations.md#unreadable-operation-state) | the read succeeds | condition |
| `resolver_failed` | starting the resolver fails or does not finish in time | an iteration finds the resolver running | condition |
| `caddy_failed` | bringing up the proxy fails or does not finish in time, or a [proxy upgrade](proxy.md#proxy-upgrades) is rejected | an iteration finds the proxy healthy | condition |
| `routes_failed` | applying the host's routes fails | the routes are next applied | condition |
| `nftables_failed` | applying the host's packet-filtering rules fails | the rules are next applied | condition |
| `proxy_failed` | applying the proxy configuration fails | a proxy configuration is next applied | condition |
| `tailscale_unreachable` | the Tailscale discovery provider has failed 5 consecutive polls | a poll succeeds | condition |
| `reconciler_panic` | an iteration of the reconciliation loop panics | an operator clears it | event |

The runtime MUST file `app_priorities_unreadable` and `scaling_decisions_unreadable` on the first failed read, since the runtime holds reconciliation while either persists.
A withheld apply MUST neither file nor clear `routes_failed`, `nftables_failed` or `proxy_failed`.
A `container_runtime_unavailable` fault MUST name the unavailable backend and state that no workloads will be actuated until the runtime is restarted with working backends.
A `resolver_failed` or `caddy_failed` fault MUST name the failure.

## Reporting faults

The runtime MUST file and clear the faults below against `seedling`, with an empty subject.

| kind | filed when | clears when | shape |
| --- | --- | --- | --- |
| `canopy_report_failed` | a [report](canopy-reporting.md#report-failure-fault) to Canopy fails while an offer is live | a later report succeeds, Canopy access is disabled, or no offer is live | condition |
| `audit_lag` | the audit log writer falls behind and drops events | an operator clears it | event |
| `audit_log_unavailable` | the runtime cannot open the [audit log](audit-log.md#write-failures) | the runtime opens it | condition |

A `canopy_report_failed` fault MUST carry the latest failure.
An `audit_lag` fault MUST state how many events were dropped.
An `audit_log_unavailable` fault MUST state how many events went unwritten while it was active.
