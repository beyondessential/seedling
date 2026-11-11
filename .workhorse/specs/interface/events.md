---
id: EVT
---

# Event feed

Applies to the event feed, through which the [Operator Interface](../overview.md#operator-interface) pushes a [host](../overview.md#host)'s changes to subscribed clients.

## Subscription

`/events/subscribe` is a [subscription request](wire.md#subscription-requests) that takes no parameters.
On success the result is `{}`, and the server then delivers events on a server-initiated unidirectional stream as [event stream framing](wire.md#subscription-requests) specifies, for as long as the connection lasts.

## Event objects

An event is a JSON object.
Every event MUST carry these members:

| member | type | meaning |
| --- | --- | --- |
| `type` | string | the event's type, from the [event types](#event-types) |
| `timestamp` | string | when the event occurred, as an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) timestamp |

When a request on the Operator Interface causes an event, the event MUST carry an `actor` member holding the [actor](wire.md#actor-attribution) resolved from that request.
An event the runtime causes on its own, such as an operation with the trigger `boot` or `schedule`, MUST NOT carry `actor`.

A client MUST ignore members it does not recognise, and SHOULD ignore events whose `type` it does not recognise.

## Common members

Members of the same name carry the same type and meaning across event types.

| member | type | meaning |
| --- | --- | --- |
| `app` | string | an [app](../overview.md#app)'s name |
| `generation`, `previous_generation` | integer | an app's [generation](../overview.md#generation) after and before the change |
| `source_generation`, `target_generation` | integer | an operation's [source and target generations](../runtime/operations.md#initiating-events) |
| `action_name` | string | the name of the action an operation runs |
| `operation_id` | string | a [lifecycle operation](../overview.md#lifecycle-operation)'s identifier |
| `session_id` | string | a [shell session](shells.md#shell-session)'s identifier |
| `forward_id` | string | a [port forward](port-forwards.md#port-forward)'s identifier |
| `offer_id` | string | a Canopy offer's identifier |
| `held_id` | string | a [held volume](../runtime/actuation.md#held-volume)'s identifier |
| `name` | string | the name of the thing the event is about |
| `external_name` | string | the name of an app's external volume or external service slot |
| `deployment` | string | a deployment's name |
| `description` | string | text an operator supplied |

A volume target is a [volume reference](shells.md#volume-reference) with `kind` `"site"` or `"app"`.
A service target is a JSON object with `kind` `"site"` and `name`, naming a site service, or with `kind` `"app"`, `app` and `service`, naming a service an app declares.

## Event types

The event types and their members beyond `type`, `timestamp` and `actor` are below.
A member marked "optional" is absent where it has no value.

### App events

| `type` | members | emitted |
| --- | --- | --- |
| `AppRegistered` | `app`, `generation` | as [registration](apps.md#registration) specifies |
| `AppDeregistered` | `app` | as [deregistration](apps.md#removal) specifies |
| `AppUpdated` | `app`, `generation`, `previous_generation` (integer or null), `definition` (object); and `name`, `previous_value`, `new_value` when a parameter changed alongside | as [definition update](apps.md#update) specifies |
| `AppPhaseChanged` | `app`, `phase` | on every change of an app's phase |
| `AppPriorityChanged` | `app`, `priority` (string), `previous_priority` (string) | as [priority change](app-control.md#app-priority) specifies |
| `ParamSet` | `app`, `name`, `previous_value` (string, optional), `new_value` (string, optional), `redacted` (`true`, optional), `generation`, `previous_generation` | as [setting a parameter](params.md#parameter-set) specifies |
| `ParamUnset` | `app`, `name`, `previous_value` (string, optional), `redacted` (`true`, optional), `generation`, `previous_generation` | as [unsetting a parameter](params.md#parameter-unset) specifies |
| `ScaleChanged` | `app`, `deployment`, `scale`, `previous_scale`, `bounds_low`, `bounds_high` (integers) | when a deployment's scale changes, by [operator request](app-control.md#scaling) or [autonomously](../runtime/autonomous.md#scale) |
| `DeploymentRestarted` | `app`, `deployment`, `operation_id` | as [deployment restart](app-control.md#deployment-restart) specifies |
| `ResourceStopped` | `app`, `kind` (string), `name` | as [stopping a resource](app-control.md#resource-stop) specifies |
| `ResourceUnstopped` | `app`, `kind` (string), `name` | as [unstopping a resource](app-control.md#resource-stop) specifies |

`definition` in `AppUpdated` is the [definition provenance](definitions.md#provenance-reporting) of the new definition.
In `AppUpdated`, `previous_value` and `new_value` MUST be `null` for an unset value and for a secret parameter.
`phase` in `AppPhaseChanged` is one of `"not_installed"`, `"installing"`, `"installed"` and `"uninstalling"`.
In `ParamSet` and `ParamUnset`, a secret parameter's event MUST carry `redacted` `true` and no `previous_value` or `new_value`, as [secret parameter events](params.md#parameter-storage) require.

### Operation events

| `type` | members | emitted |
| --- | --- | --- |
| `OperationStarted` | `app`, `action_name`, `operation_id`, `source_generation`, `target_generation`, `trigger` (string) | when a lifecycle operation starts |
| `OperationCompleted` | `app`, `action_name`, `operation_id`, `source_generation`, `target_generation` | when a lifecycle operation completes successfully |
| `OperationFailed` | `app`, `action_name`, `operation_id`, `source_generation`, `target_generation`, `error` (string) | when a lifecycle operation fails or is cancelled |

`trigger` is the operation's [initiating trigger](../runtime/operations.md#initiating-events).

### Fault and resource events

| `type` | members | emitted |
| --- | --- | --- |
| `FaultFiled` | `id`, `app`, `resource_type`, `resource_name`, `instance_id`, `kind`, `description` | when a fault is filed |
| `FaultCleared` | `id`, `app`, `kind` | when a fault is cleared |
| `ResourceStateChanged` | `app`, `resource_type` (string), `resource_name` (string), `instance_id` (string), `state` (string) | when a resource instance's derived lifecycle state changes |

`FaultFiled` carries the members of a [fault record](faults.md#fault-records), except that the event's `timestamp` is the record's.
In `ResourceStateChanged`, `resource_type` is the resource's [type name](wire.md#resource-type-names), and `state` is the instance's new [lifecycle state](../overview.md#lifecycle-state), named as the lifecycle states are.

### Session events

| `type` | members | emitted |
| --- | --- | --- |
| `ShellStarted` | `session_id`, `app`, `name` | as [shell session events](shells.md#session-events) specifies |
| `ShellExited` | `session_id`, `exit_code` (integer) | as [shell session events](shells.md#session-events) specifies |
| `ForwardStarted` | `forward_id`, `app`, `service` (string), `port` (integer) | as [forward events](port-forwards.md#forward-events) specifies |
| `ForwardStopped` | `forward_id` | as [forward events](port-forwards.md#forward-events) specifies |
| `CanopyOffered` | `offer_id`, `agent` (string), `endpoint` (string) | as [Canopy offers](canopy-relay.md#offers) specifies |
| `CanopyWithdrawn` | `offer_id`, `reason` (string) | as [Canopy offer lifetime](canopy-relay.md#offer-lifetime) specifies |

`reason` in `CanopyWithdrawn` is one of the reasons [offer lifetime](canopy-relay.md#offer-lifetime) defines.

### Site resource events

| `type` | members | emitted |
| --- | --- | --- |
| `HeldVolumeCreated` | `held_id`, `app`, `volume_name` (string), `reason` (string) | as [held volumes](../runtime/actuation.md#held-volumes) specifies |
| `HeldVolumeDeleted` | `held_id` | as [held volumes](../runtime/actuation.md#held-volumes) specifies |
| `HeldVolumeRestored` | `held_id`, `site_name` (string) | as [held volumes](../runtime/actuation.md#held-volumes) specifies |
| `SiteVolumeCreated` | `name`, `kind` (string), `host_path` (string, optional) | as [site volume events](../runtime/site-resources.md#site-volumes) specifies |
| `SiteVolumeDeleted` | `name`, `kind` (string), `held_id` (optional) | as [site volume events](../runtime/site-resources.md#site-volumes) specifies |
| `SiteVolumeSnapshotted` | `name`, `source` (volume target) | as [snapshot events](../runtime/site-resources.md#site-volume-snapshots) specifies |
| `SiteVolumePromoted` | `name`, `source` (string) | as [promotion events](../runtime/site-resources.md#snapshot-promotion) specifies |
| `ExternalVolumeMapped` | `app`, `external_name`, `target` (volume target), `read_only` (boolean) | as [external volume mapping events](../runtime/site-resources.md#external-mappings) specifies |
| `ExternalVolumeUnmapped` | `app`, `external_name` | as [external volume mapping events](../runtime/site-resources.md#external-mappings) specifies |
| `ExternalVolumeRemapped` | `app`, `external_name`, `target` (volume target), `read_only` (boolean), `previous_target` (volume target), `previous_read_only` (boolean) | as [external volume mapping events](../runtime/site-resources.md#external-mappings) specifies |
| `SiteServiceCreated` | `name`, `description` (optional) | as [site service events](../runtime/site-resources.md#site-service-lifecycle) specifies |
| `SiteServiceDeleted` | `name` | as [site service events](../runtime/site-resources.md#site-service-lifecycle) specifies |
| `SiteServiceEndpointAdded` | `name`, `service_port` (integer), `protocol` (string), `remote_host` (string), `remote_port` (integer) | as [site service events](../runtime/site-resources.md#site-service-lifecycle) specifies |
| `SiteServiceEndpointRemoved` | `name`, `service_port` (integer), `protocol` (string), `remote_host` (string), `remote_port` (integer) | as [site service events](../runtime/site-resources.md#site-service-lifecycle) specifies |
| `ExternalServiceMapped` | `app`, `external_name`, `target` (service target) | as [external service mapping events](../runtime/site-resources.md#external-mappings) specifies |
| `ExternalServiceUnmapped` | `app`, `external_name` | as [external service mapping events](../runtime/site-resources.md#external-mappings) specifies |
| `ExternalServiceRemapped` | `app`, `external_name`, `target` (service target), `previous_target` (service target) | as [external service mapping events](../runtime/site-resources.md#external-mappings) specifies |
| `SiteIngressCreated` | `name`, `hostname` (string), `source` (string), `discovered_provider` (string, optional), `tls_provider` (string), `description` (optional) | as [site ingress events](../runtime/site-resources.md#attachments) specifies |
| `SiteIngressUpdated` | `name`, `hostname` (string), `tls_provider` (string), `description` (optional) | as [site ingress events](../runtime/site-resources.md#attachments) specifies |
| `SiteIngressDeleted` | `name`, `source` (string) | as [site ingress events](../runtime/site-resources.md#attachments) specifies |
| `SiteIngressAttachmentAdded` | `name`, `port` (integer), `protocol` (string), `target_kind` (string), `target_app` (string, optional), `target_service` (string, optional), `redirect_url` (string, optional), `redirect_code` (integer, optional) | as [site ingress events](../runtime/site-resources.md#attachments) specifies |
| `SiteIngressAttachmentUpdated` | as `SiteIngressAttachmentAdded` | as [site ingress events](../runtime/site-resources.md#attachments) specifies |
| `SiteIngressAttachmentRemoved` | `name`, `port` (integer), `protocol` (string) | as [site ingress events](../runtime/site-resources.md#attachments) specifies |
| `TemplateCreated` | `name` | as [template creation](templates.md#creation) specifies |
| `TemplateUpdated` | `name` | as [template update](templates.md#update) specifies |
| `TemplateRemoved` | `name` | as [template removal](templates.md#removal) specifies |
| `TemplateInstantiated` | `template` (string), `app` | as [template instantiation](templates.md#instantiation) specifies |

In `SiteVolumeCreated`, `kind` is `"managed"` or `"bind"`, and `host_path` is present for a bind site volume.
In `SiteVolumeDeleted`, `kind` is `"managed"`, `"bind"` or `"snapshot"`, and `held_id` is present where the deletion made a held volume.
In the site ingress events, `source` is `"manual"` or `"discovered"`, `discovered_provider` is present for a discovered ingress, and `tls_provider` is `"acme"`, `"tailscale"`, `"internal"` or `"none"`.
In the attachment events, `target_kind` is `"forward"` or `"redirect"`; a forward carries `target_app` and `target_service`, and a redirect carries `redirect_url` and `redirect_code`.

### Server events

| `type` | members | emitted |
| --- | --- | --- |
| `ServerBusy` | `reason` (string) | when the server refuses a stream at its [concurrency limit](wire.md#stream-limit) |
| `EventsDropped` | `skipped` (integer, optional) | as [gaps](#gaps) specifies |

## Ordering

The server MUST deliver the events on one connection's event stream in the order they occurred.

> [!NOTE]
> Events on separate connections carry no ordering relative to each other.

## Gaps

When a subscriber falls far enough behind that the server skips events rather than deliver them, the server MUST write an `EventsDropped` event to that subscriber's stream at the point of the gap.
`skipped` MUST give the number of events skipped where the server knows it.
The server MUST write `EventsDropped` only to the subscriber that fell behind.
A subscriber that receives `EventsDropped` MUST treat any state it keeps current from the feed as stale and read that state afresh.
