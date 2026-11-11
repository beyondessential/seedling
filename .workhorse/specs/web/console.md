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

The console MUST let the operator list apps with their statuses, and see an app's status with its resources, faults, parameters, install requirements and actions.
It MUST let the operator register an app with a script, update an app's script or fetch its definition from a registry, and deregister an app.
It MUST let the operator set and unset an app's parameters, scale its deployments, set its priority, view its generation history, and plan a change.
It MUST let the operator invoke an app's lifecycle actions, and invoke its install action with the install requirements.

The console MUST let the operator list the authorised client keys, authorise a key, and revoke a key.
It MUST let the operator list, add and remove the hostnames on the container registry allowlist.

The console MUST let the operator list manual and discovered site ingresses, each kind apart from the other.
It MUST let the operator create, update and delete manual site ingresses.
It MUST let the operator attach and detach forward and redirect targets on manual and discovered site ingresses.
It MUST offer to delete each discovered site ingress that is [stale](../runtime/site-resources.md#sources).
It MUST show the status of each ingress discovery provider.
While ingress discovery is unhealthy, the console MUST warn the operator.

The console MUST let the operator list, create and delete site volumes of the managed, bind and snapshot kinds.
It MUST list the volumes apps export.
It MUST let the operator list, add, remap and remove external volume mappings.
It MUST list held volumes, and let the operator confirm a held volume's deletion.

The console MUST show the operator the error of each request that fails.
It MUST NOT present a read that failed as an empty result.

## Signing in

The console MUST ask the operator for a password when the web interface [asks for one](serving.md#authentication).
When the web interface answers that no credential would authenticate, the console MUST show that sign-in is unavailable, and MUST NOT ask for a password.

## Guarded controls

A console MUST start in the `read` safety mode.
A console reloaded in the same browser tab MAY resume an elevation that has not expired.

A control that changes host state MUST require at least the `write` mode.
A control that removes, revokes, stops, cancels or clears something MUST require the `dangerous` mode, with two exceptions.
A control that clears [faults](../overview.md#fault) MUST require the `write` mode.
Closing a shell the console itself opened, which stops it, MUST be available in every safety mode.
A control that requires a mode above the console's current one MUST be disabled, and SHOULD show the mode it requires.

> [!NOTE]
> Clearing a fault removes only its record, and a fault whose condition still holds is filed again.

Elevating to `dangerous` MUST require the operator's confirmation.
An elevation to `write` or `dangerous` MUST revert to `read` no later than ten minutes after it began, and the console SHOULD show the time remaining.
The console reports its safety mode to the web interface as [session tracking](serving.md#session-tracking) specifies.

The console MUST flag its count of [web sessions](serving.md#web-session) and its safety mode control whenever any other web session is in the `write` or `dangerous` mode, whatever the console's own mode.
The flag MUST show, in the colour the console gives that mode, the highest mode any other web session is in.

> [!NOTE]
> The safety mode guards against slips, not against a hostile operator.
> The web interface relays every request whatever mode a console reports.

## Event feed

The console MUST subscribe to the [event feed](serving.md#event-relay) for as long as its web session is open, and show the operator events as they arrive.
A view the console keeps current from the feed MUST update without polling and without a reload.
On receiving an `EventsDropped` event, the console MUST read afresh every view it keeps current from the feed.

## App definitions

The app detail view MUST show the source of the app's current definition, from its [definition provenance](../interface/definitions.md#provenance-reporting).
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

The apps list MUST show, for each app with active faults, how many there are, beside the app's status and in the colour the console uses for faults.
It MUST NOT show a fault count for an app with none.

The app detail view MUST show the app's [Canopy application type](../language/app.md#canopy-type) when its definition declares one.

For each Deployment with a declared [healthcheck](../language/deployment.md#health-checks), the app detail view MUST show, beside the Deployment's lifecycle state, that it declares one, its `on_failure` response, and its current check state: passing, failing, or within its start period.
The console MUST show a passing check in its success colour, a failing check in its error colour, and a check within its start period in its warning or neutral colour.
On hover or focus, the console MUST reveal the healthcheck's `kind`, its `on_failure` response and, for a command healthcheck, a truncated form of its command.

## Priority

The app detail view MUST show the app's [priority](../interface/app-control.md#app-priority) and let the operator set it to any level the Operator Interface accepts.
The apps list MUST mark each app whose priority is not `normal`, beside its status, and MUST NOT mark an app at `normal`.
The mark MUST distinguish a raised priority from a lowered one by colour, following the status colours the console uses elsewhere.
A change of priority MUST appear in both views without a reload.

For each Deployment declaring a [priority](../language/deployment.md#priority) other than `normal`, the app detail view MUST show the declared level beside the Deployment's lifecycle state, as configuration, not as a control.
That indicator MUST use the same colours as the app priority mark, so that a level reads alike in both.
That indicator MUST read as standing within the app, not across apps.

> [!NOTE]
> [Kill order](../runtime/priority.md#kill-order) is app-major, so a `Critical` Deployment in a `low`-priority app is shed before any workload of a `high`-priority app.

## Shells

The console MUST offer each shell an app defines, as an action of kind `shell` in its [description](../interface/apps.md#description), and MUST collect values for the shell's declared parameters before opening it.
It MUST offer a [volume shell](../interface/shells.md#volume-shells) over a single volume from each site volume and each app volume resource it lists, and MUST open it without asking the operator for anything further.
The console MUST label a volume shell with the volume's [display name](../interface/shells.md#volume-shells).

The console MUST show open shells in a persistent sidebar on the left of its layout, one tab per shell, and MUST let several shells be open at once.
It MUST fit each terminal to the space it has and send [`/shells/resize`](../interface/shells.md#resizing-a-terminal) when that space changes, with at most one resize request in flight for a shell at a time.
When a shell exits, the console MUST show its exit code and offer to close or reopen it.
When the operator closes a shell that has not exited, the console MUST stop it with [`/shells/stop`](../interface/shells.md#stopping-a-session).

Volume shells MUST be available in every safety mode.
In the `read` mode the console MUST open a volume shell with `read_only: true`, and MUST mark its label as read-only.
In the `write` and `dangerous` modes the console MUST open a volume shell read-write.
The safety mode in force when the operator opens a shell MUST govern its mounts for the shell's lifetime.

## Images

The console MUST offer an images view at the path `/images`.
It MUST show every image in the host's local storage, with its references, size, last use, and whether it is pinned or in use, and let the operator remove it.
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

The console MUST offer a restart history view at the path `/restarts`.
It MUST list [restart records](../interface/faults.md#restart-records), most recent first, with their app, instance, time and exit status, and let the operator filter them by app.
It MUST distinguish deliberate restarts from recovery restarts.
It MUST show the crash-loop rate threshold and window, and let the operator change them.

## Certificates

The console MUST offer a certificates view at the path `/certificates`.
It MUST let the operator list per-hostname certificate policies, clear one, and add one binding a hostname or wildcard either to ACME DNS-01 through a configured DNS provider or to the Canopy-only policy.
It MUST let the operator view and edit the ACME contact email and the certificate profile used for issuance.
It MUST let the operator list DNS providers without their credentials, add a Route 53 provider, and delete a provider.
For each hostname, it MUST show the strategy responsible for it and when that strategy falls through to the next, as [hostname listing](../interface/tls.md#hostname-view) reports, with Canopy certificates among the others.

The console MUST group stored certificates by label and show each one's state, origin, issuer, expiry and serial.
It MUST flag a self-signed certificate, and one that expires within fourteen days.
For a certificate obtained through a certificate signing request, the console MUST show the hostnames requested where they differ from the certificate's primary subject alternative name, and MUST flag the certificate, naming the missed hostnames, when it does not cover them all.

A certificate signing request the console begins MUST take one or more hostnames, each a hostname or a `*.<suffix>` wildcard, as [beginning a request](../interface/tls.md#certificate-signing-requests) accepts.
For an active manual or request-derived certificate that expires within fourteen days or has expired, the console MUST offer to [renew](../runtime/tls/certificates.md#certificate-signing-requests) it.
To renew, the console MUST begin a certificate signing request pre-filled, editably, with the certificate's DNS subject alternative names and the hostnames it currently serves.
Where the certificate already has a pending renewal, the console MUST lead to that renewal instead, and MUST name the certificate a pending renewal renews.

> [!NOTE]
> The TLS secrets the Operator Interface withholds, and the console therefore never receives, are set by [TLSM](../interface/tls.md#conventions).

## Canopy access

The console MUST offer a Canopy view at the path `/canopy`.
It MUST show whether Canopy access is enabled, and let the operator turn it on or off.
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
The console MUST show the number of open web sessions on every view, and keep it current without a reload.
The console MUST keep the view current from the `WebSessionStarted`, `WebSessionStopped`, `WebSessionModeChanged`, `ShellStarted`, `ShellExited`, `ForwardStarted` and `ForwardStopped` events, without polling.
