---
id: UI
---

# Operator console

Applies to the browser application through which an [operator](../overview.md#operator) observes and controls a [host](../overview.md#host) by way of the [web interface](serving.md#web-interface).

## Terminology

### Console

The single-page application the web interface serves, through which an operator works with a host from a browser.

### Safety mode

The tier of controls a console currently enables: `read`, `write` or `dangerous`, in increasing order.

## Operator capabilities

The console MUST let an operator observe and control a host through its [Operator Interface](../overview.md#operator-interface) from a browser, without a terminal.
It SHOULD offer each Operator Interface operation the CLI offers, other than those this spec withholds and those a browser cannot carry out, such as listening on a local port for a port forward.
It MUST offer log streaming for both app workloads and the host's infrastructure components.

The console MUST show the operator the error of each request that fails.
It MUST NOT present a read that failed as an empty result.
While ingress discovery is unhealthy, the console MUST warn the operator.

## Guarded controls

A console MUST start in the `read` safety mode.
A console reloaded in the same browser tab MAY resume an elevation that has not expired.

A control that changes host state MUST require at least the `write` mode.
A control that removes, revokes, stops, cancels or clears something SHOULD require the `dangerous` mode.
A control that requires a mode above the console's current one MUST be disabled, and SHOULD show the mode it requires.

Elevating to `dangerous` MUST require the operator's confirmation.
An elevation to `write` or `dangerous` MUST revert to `read` no later than ten minutes after it began, and the console SHOULD show the time remaining.
The console reports its safety mode to the web interface as [session tracking](serving.md#session-tracking) specifies.

The console MUST show the operator when any other web session is in the `write` or `dangerous` mode, distinguishing the two, whatever the console's own mode.

> [!NOTE]
> The safety mode guards against slips, not against a hostile operator.
> The web interface relays every request whatever mode a console reports.

## Event feed

The console MUST subscribe to the [event feed](serving.md#event-relay) for as long as its web session is open, and show the operator events as they arrive.
A view the console keeps current from the feed MUST update without polling and without a reload.
On receiving an `EventsDropped` event, the console MUST read afresh every view it keeps current from the feed.

## App definitions

The app detail view MUST show the source of the app's current definition, from its [provenance](../interface/definitions.md#provenance-reporting).
For a fetched definition, it MUST show the reference and digest.
For a pushed definition, it MUST show the content hash and the actor who pushed it, with any origin the client reported, marked as reported by the client.

The console MUST let the operator update an app's definition from a registry.
For an app whose definition was fetched, it MUST offer the [tags](../interface/definitions.md#repository-tags) of the recorded reference's repository to pick from, and the operator MAY instead enter a complete reference.
The operator MAY change one parameter alongside the new definition.
Before the update is applied, the console MUST show the [planned change](../interface/params.md#change-planning), including any validator rejections, and MUST NOT apply a change that is rejected.
The console MUST issue the update as a single [`/apps/update`](../interface/apps.md#update) request.
While the app has an active `definition_source_moved` fault, the console MUST show beside the provenance that the recorded tag has moved, and offer that tag to update to.

The console MUST let the operator edit a definition whose bundle has exactly one script file.
An edit MUST carry every other file in the bundle over unchanged, and is recorded as pushed.
A definition whose script spans several files MUST be shown read-only.
The console MUST NOT apply an edit until it has read the definition's files, and MUST show the reason when it cannot read them.

## App status

The apps list MUST show, for each app with active faults, how many there are.

The app detail view MUST show the app's [Canopy application type](../language/app.md#canopy-type) when its definition declares one.

For each container resource with a declared [healthcheck](../language/deployment.md#health-checks), the app detail view MUST show its `on_failure` response and its current check state: passing, failing, or within its start period.
It SHOULD reveal the healthcheck's `kind` and, for a command healthcheck, its command, on hover or focus.

## Priority

The app detail view MUST show the app's [priority](../interface/app-control.md#app-priority) and let the operator set it to any level the Operator Interface accepts.
The apps list MUST mark each app whose priority is not `normal`, distinguishing a raised priority from a lowered one.
A change of priority MUST appear in both views without a reload.

For each Deployment declaring a [priority](../language/deployment.md#priority) other than `normal`, the app detail view MUST show the declared level as configuration, not as a control.
That indicator MUST read as standing within the app, not across apps.

> [!NOTE]
> [Kill order](../runtime/priority.md#kill-order) is app-major, so a `Critical` Deployment in a `low`-priority app is shed before any workload of a `high`-priority app.

## Shells

The console MUST offer each shell an app defines, as an action of kind `shell` in its [description](../interface/apps.md#description), and MUST collect values for the shell's declared parameters before opening it.
It MUST offer a [volume shell](../interface/shells.md#volume-shells) over a single volume from each site volume and each app volume resource it lists, and SHOULD label the shell with the volume's display name.

The console MUST let several shells be open at once.
It MUST fit each terminal to the space it has and send [`/shells/resize`](../interface/shells.md#resizing-a-terminal) when that space changes, with at most one resize request in flight for a shell at a time.
When a shell exits, the console MUST show its exit code and offer to reopen it.
When the operator closes a shell that has not exited, the console MUST stop it with [`/shells/stop`](../interface/shells.md#stopping-a-session).

Volume shells MUST be available in every safety mode.
In the `read` mode the console MUST open a volume shell with `read_only: true`, and SHOULD mark it as read-only.
The safety mode in force when the operator opens a shell MUST govern its mounts for the shell's lifetime.

## Images

The console MUST show every image in the host's local storage, with its references, size, last use, and whether it is pinned or in use, and let the operator remove it.
It MUST show the host's image pins, with the pinning app, reference and time pinned, and let the operator clear each.
It MUST let the operator remove every image that is not backing a running container in one action.

The app detail view MUST show the images the app's running containers use and the images the app has pinned, let the operator remove each, and clear all of the app's pins in one action.
It MUST offer to [discover](../interface/images.md#image-discovery) the app's images from its handlers in lenient mode, and show each discovered reference as potentially used, distinct from in use and pinned.
When a handler's probe reports an error or is skipped, the console MUST show it beside the results.
The console MUST let the operator pull and pin, for the app, every discovered reference not already present or pinned.

Before removing an image, the console MUST ask the operator to confirm, stating that removal fails while the image is in use.
The console MUST NOT offer to force an image's removal, nor to pull a reference the operator supplies.

## Volumes

Before the console deletes a volume or confirms the deletion of a held volume, it MUST ask the operator to confirm, stating the consequence for that kind of volume:

| action | consequence |
| --- | --- |
| confirming deletion of a held volume | its data is removed permanently |
| deleting a managed site volume | its data is held, awaiting the operator's confirmation |
| deleting a snapshot site volume | the snapshot's data is removed permanently, and its source volume is untouched |
| deleting a bind site volume | only the reference is dropped, and the host path is untouched |

The console MUST show the number of held volumes on every view, and keep it current without a reload.

## Restart history

The console MUST list [restart records](../interface/faults.md#restart-records), most recent first, with their app, instance, time, initiator and exit status, and let the operator filter them by app.
It MUST distinguish deliberate restarts from recovery restarts.
It MUST show the crash-loop rate threshold and window, and let the operator change them.

## Certificates

The console MUST let the operator manage per-hostname certificate policies, the ACME contact email and certificate profile, stored certificates, and DNS providers.
For each hostname, it MUST show the strategy responsible for it and when that strategy falls through to the next, as [hostname listing](../interface/tls.md#hostname-view) reports, with Canopy certificates among the others.

The console MUST group stored certificates by label and show each one's state, origin, issuer, expiry and serial.
It MUST flag a self-signed certificate, and one that expires within fourteen days.
For a certificate obtained through a certificate signing request, the console MUST show the hostnames requested where they differ from the certificate's primary subject alternative name, and MUST flag the certificate, naming the missed hostnames, when it does not cover them all.

A certificate signing request the console begins MUST take one or more hostnames, each a hostname or a `*.<suffix>` wildcard, as [beginning a request](../interface/tls.md#certificate-signing-requests) accepts.
For an active manual or request-derived certificate that expires within fourteen days or has expired, the console MUST offer to [renew](../runtime/tls/certificates.md#certificate-signing-requests) it.
Renewing begins a certificate signing request pre-filled, editably, with the certificate's DNS subject alternative names and the hostnames it currently serves.
Where the certificate already has a pending renewal, the console MUST lead to that renewal instead, and MUST name the certificate a pending renewal renews.

## Canopy access

The console MUST show whether Canopy access is enabled, and let the operator turn it on or off.
It MUST show the offer that would serve the next request, with its agent, endpoint and time offered, or that no client is offering one.
It MUST show the outcome and time of the most recent status report, with its error when it failed, and each app reported to Canopy with its type.

The console MUST NOT offer to register or withdraw an offer, to relay a request, or to force a status report.

> [!NOTE]
> Offers come from the client that carries the requests, not from an operator.
> A control relaying an arbitrary request would give any operator the full authority of that client's Canopy identity.

## Connected clients

The console MUST show the open web sessions, shell sessions and port forwards, and the recently active actors, that [`/connected-clients/list`](serving.md#connected-clients) reports.
Each entry MUST show its actor and when it connected or opened, with the app for a shell or forward.
A web session entry MUST also show its `last_seen` and its safety mode.
The console MUST keep the view current from the `WebSessionStarted`, `WebSessionStopped`, `WebSessionModeChanged`, `ShellStarted`, `ShellExited`, `ForwardStarted` and `ForwardStopped` events, without polling.
