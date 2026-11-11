---
id: EVT
---

# Event feed

Applies to the event feed, through which the [Operator Interface](../overview.md#operator-interface) pushes a [host](../overview.md#host)'s changes to subscribed clients.

## Subscription

`/events/subscribe` is a [subscription request](rule:i/stream.subscribe) that takes no parameters.
On success the result is `{}`, and the server then delivers events on a server-initiated unidirectional stream as [event stream framing](rule:i/stream.events) specifies, for as long as the connection lasts.

## Event objects

An event is a JSON object.
Every event MUST carry these members:

| member | type | meaning |
| --- | --- | --- |
| `type` | string | the event's type, from the [event types](#event-types) |
| `timestamp` | string | when the event occurred, as an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) timestamp |

When a request on the Operator Interface causes an event, the event MUST carry an `actor` member holding the [actor](rule:i/wire.actor) resolved from that request.
An event the runtime causes on its own, such as an operation with the trigger `boot` or `schedule`, MUST NOT carry `actor`.

A client MUST ignore members it does not recognise, and SHOULD ignore events whose `type` it does not recognise.

## Common members

Members of the same name carry the same type and meaning across event types.

| member | type | meaning |
| --- | --- | --- |
| `app` | string | an [app](../overview.md#app)'s name |
| `generation`, `previous_generation` | integer | an app's [generation](../overview.md#generation) after and before the change |
| `source_generation`, `target_generation` | integer | an operation's [source and target generations](rule:r/operation.lifecycle.generations) |
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
| `AppRegistered` | `app`, `generation` | as [registration](rule:i/app.register) specifies |
| `AppDeregistered` | `app` | as [deregistration](rule:i/app.deregister) specifies |
| `AppUpdated` | `app`, `generation`, `previous_generation` (integer or null), `definition` (object); and `name`, `previous_value`, `new_value` when a parameter changed alongside | as [definition update](rule:i/app.update) specifies |
| `AppPhaseChanged` | `app`, `phase` | on every change of an app's phase |
| `AppPriorityChanged` | `app`, `priority` (string), `previous_priority` (string) | as [priority change](rule:i/app.priority.set) specifies |
| `ParamSet` | `app`, `name`, `previous_value` (string, optional), `new_value` (string, optional), `redacted` (`true`, optional), `generation`, `previous_generation` | as [setting a parameter](rule:i/param.set) specifies |
| `ParamUnset` | `app`, `name`, `previous_value` (string, optional), `redacted` (`true`, optional), `generation`, `previous_generation` | as [unsetting a parameter](rule:i/param.unset) specifies |
| `ScaleChanged` | `app`, `deployment`, `scale`, `previous_scale`, `bounds_low`, `bounds_high` (integers) | when a deployment's scale changes, by [operator request](rule:i/scale.set) or [autonomously](rule:r/autonomous.scale) |
| `DeploymentRestarted` | `app`, `deployment`, `operation_id` | as [deployment restart](rule:i/deployment.restart) specifies |
| `ResourceStopped` | `app`, `kind` (string), `name` | as [stopping a resource](rule:i/resource.stop) specifies |
| `ResourceUnstopped` | `app`, `kind` (string), `name` | as [unstopping a resource](rule:i/resource.unstop) specifies |

`definition` in `AppUpdated` is the new definition's [provenance](rule:i/definition.provenance).
In `AppUpdated`, `previous_value` and `new_value` MUST be `null` for an unset value and for a secret parameter.
`phase` in `AppPhaseChanged` is one of `"not_installed"`, `"installing"`, `"installed"` and `"uninstalling"`.
In `ParamSet` and `ParamUnset`, a secret parameter's event MUST carry `redacted` `true` and no `previous_value` or `new_value`, as [secret parameter events](rule:i/param.store.secret) require.

### Operation events

| `type` | members | emitted |
| --- | --- | --- |
| `OperationStarted` | `app`, `action_name`, `operation_id`, `source_generation`, `target_generation`, `trigger` (string) | when a lifecycle operation starts |
| `OperationCompleted` | `app`, `action_name`, `operation_id`, `source_generation`, `target_generation` | when a lifecycle operation completes successfully |
| `OperationFailed` | `app`, `action_name`, `operation_id`, `source_generation`, `target_generation`, `error` (string) | when a lifecycle operation fails or is cancelled |

`trigger` is the operation's [initiating trigger](rule:r/operation.lifecycle.events).

### Fault and resource events

| `type` | members | emitted |
| --- | --- | --- |
| `FaultFiled` | `id`, `app`, `resource_type`, `resource_name`, `instance_id`, `kind`, `description` | when a fault is filed |
| `FaultCleared` | `id`, `app`, `kind` | when a fault is cleared |
| `ResourceStateChanged` | `app`, `resource_type` (string), `resource_name` (string), `instance_id` (string), `state` (string) | when a resource instance's derived lifecycle state changes |

`FaultFiled` carries the members of a [fault record](faults.md#fault-records), except that the event's `timestamp` is the record's.
In `ResourceStateChanged`, `resource_type` is the resource's kind in lower case, and `state` is the instance's new [lifecycle state](../overview.md#lifecycle-state), named as the lifecycle states are.

### Session events

| `type` | members | emitted |
| --- | --- | --- |
| `ShellStarted` | `session_id`, `app`, `name` | as [shell session events](shells.md#session-events) specifies |
| `ShellExited` | `session_id`, `exit_code` (integer) | as [shell session events](shells.md#session-events) specifies |
| `ForwardStarted` | `forward_id`, `app`, `service` (string), `port` (integer) | as [forward events](port-forwards.md#forward-events) specifies |
| `ForwardStopped` | `forward_id` | as [forward events](port-forwards.md#forward-events) specifies |
| `CanopyOffered` | `offer_id`, `agent` (string), `endpoint` (string) | as [Canopy offers](rule:i/canopy.offer) specifies |
| `CanopyWithdrawn` | `offer_id`, `reason` (string) | as [Canopy offer lifetime](rule:i/canopy.offer.lifetime) specifies |

`reason` in `CanopyWithdrawn` is `"requested"`, `"disconnected"` or `"disabled"`.

### Site resource events

| `type` | members | emitted |
| --- | --- | --- |
| `HeldVolumeCreated` | `held_id`, `app`, `volume_name` (string), `reason` (string) | as [held volumes](../runtime/actuation.md#held-volumes) specifies |
| `HeldVolumeDeleted` | `held_id` | as [held volumes](../runtime/actuation.md#held-volumes) specifies |
| `HeldVolumeRestored` | `held_id`, `site_name` (string) | as [held volumes](../runtime/actuation.md#held-volumes) specifies |
| `SiteVolumeCreated` | `name`, `kind` (string), `host_path` (string, optional) | as [site volume events](rule:r/volume.site.lifecycle.events) specifies |
| `SiteVolumeDeleted` | `name`, `kind` (string), `held_id` (optional) | as [site volume events](rule:r/volume.site.lifecycle.events) specifies |
| `SiteVolumeSnapshotted` | `name`, `source` (volume target) | as [snapshot events](rule:r/volume.site.snapshot.events) specifies |
| `SiteVolumePromoted` | `name`, `source` (string) | as [promotion events](rule:r/volume.site.promote.events) specifies |
| `ExternalVolumeMapped` | `app`, `external_name`, `target` (volume target), `read_only` (boolean) | as [external volume mapping events](rule:r/volume.external.mapping.events) specifies |
| `ExternalVolumeUnmapped` | `app`, `external_name` | as [external volume mapping events](rule:r/volume.external.mapping.events) specifies |
| `ExternalVolumeRemapped` | `app`, `external_name`, `target` (volume target), `read_only` (boolean), `previous_target` (volume target), `previous_read_only` (boolean) | as [external volume mapping events](rule:r/volume.external.mapping.events) specifies |
| `SiteServiceCreated` | `name`, `description` (optional) | as [site service events](rule:r/service.site.lifecycle.events) specifies |
| `SiteServiceDeleted` | `name` | as [site service events](rule:r/service.site.lifecycle.events) specifies |
| `SiteServiceEndpointAdded` | `name`, `service_port` (integer), `protocol` (string), `remote_host` (string), `remote_port` (integer) | as [site service events](rule:r/service.site.lifecycle.events) specifies |
| `SiteServiceEndpointRemoved` | `name`, `service_port` (integer), `protocol` (string), `remote_host` (string), `remote_port` (integer) | as [site service events](rule:r/service.site.lifecycle.events) specifies |
| `ExternalServiceMapped` | `app`, `external_name`, `target` (service target) | as [external service mapping events](rule:r/service.external.mapping.events) specifies |
| `ExternalServiceUnmapped` | `app`, `external_name` | as [external service mapping events](rule:r/service.external.mapping.events) specifies |
| `ExternalServiceRemapped` | `app`, `external_name`, `target` (service target), `previous_target` (service target) | as [external service mapping events](rule:r/service.external.mapping.events) specifies |
| `SiteIngressCreated` | `name`, `hostname` (string), `source` (string), `discovered_provider` (string, optional), `tls_provider` (string), `description` (optional) | as [site ingress events](rule:r/ingress.site.lifecycle.events) specifies |
| `SiteIngressUpdated` | `name`, `hostname` (string), `tls_provider` (string), `description` (optional) | as [site ingress events](rule:r/ingress.site.lifecycle.events) specifies |
| `SiteIngressDeleted` | `name`, `source` (string) | as [site ingress events](rule:r/ingress.site.lifecycle.events) specifies |
| `SiteIngressAttachmentAdded` | `name`, `port` (integer), `protocol` (string), `target_kind` (string), `target_app` (string, optional), `target_service` (string, optional), `redirect_url` (string, optional), `redirect_code` (integer, optional) | as [site ingress events](rule:r/ingress.site.lifecycle.events) specifies |
| `SiteIngressAttachmentUpdated` | as `SiteIngressAttachmentAdded` | as [site ingress events](rule:r/ingress.site.lifecycle.events) specifies |
| `SiteIngressAttachmentRemoved` | `name`, `port` (integer), `protocol` (string) | as [site ingress events](rule:r/ingress.site.lifecycle.events) specifies |
| `TemplateCreated` | `name` | as [template creation](rule:i/template.create) specifies |
| `TemplateUpdated` | `name` | as [template update](rule:i/template.update) specifies |
| `TemplateRemoved` | `name` | as [template removal](rule:i/template.remove) specifies |
| `TemplateInstantiated` | `template` (string), `app` | as [template instantiation](rule:i/template.instantiate) specifies |

In `SiteVolumeCreated`, `kind` is `"managed"` or `"bind"`, and `host_path` is present for a bind site volume.
In `SiteVolumeDeleted`, `kind` is `"managed"`, `"bind"` or `"snapshot"`, and `held_id` is present where the deletion made a held volume.
In the site ingress events, `source` is `"manual"` or `"discovered"`, `discovered_provider` is present for a discovered ingress, and `tls_provider` is `"acme"`, `"tailscale"`, `"internal"` or `"none"`.
In the attachment events, `target_kind` is `"forward"` or `"redirect"`; a forward carries `target_app` and `target_service`, and a redirect carries `redirect_url` and `redirect_code`.

### Server events

| `type` | members | emitted |
| --- | --- | --- |
| `ServerBusy` | `reason` (string) | when the server refuses a stream at its [concurrency limit](rule:i/stream.concurrency-limit) |
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
