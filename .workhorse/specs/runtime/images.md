---
id: IMG
---

# Images

Applies to how the [runtime](../overview.md#runtime) pulls, protects, tracks and removes container images on the [host](../overview.md#host), and how it discovers the images an [app](../overview.md#app) may need.

## Terminology

### Image reference

The string a definition names an image by, such as a tag or a digest reference.
Two references are equal only when the strings are equal.

### Pin

A durable record that an app has asked for an image reference to be kept on the host.

### Probe

An evaluation of an app's handler closures that enumerates the image references they could pull, without acting on the host.

## Image pulls

The runtime MUST retry a failed pull.
Successive failures for one image reference MUST be spaced by an increasing delay up to a fixed maximum.
The delay SHOULD start at 5 seconds and double with each consecutive failure, up to a maximum of 300 seconds.
A failure that follows the previous failure by more than the maximum delay SHOULD start a new count.
A successful pull MUST reset the count.

However long a reference has been failing, the runtime MUST attempt it again once the maximum delay has passed.
When a reference reaches 5 consecutive failures, the runtime MUST file an [`image_pull_failed`](faults.md#workload-faults) fault and MUST keep attempting at the maximum delay.

The runtime SHOULD consider a pull that has not finished within 300 seconds stuck, and MAY start another.
The outcome of a superseded pull MUST NOT change the pacing of the pull that superseded it.

## Image warming

When an action closure calls [`rt.warm_images`](rule:l/rt.warm-images) with a selection containing container resources, the runtime MUST collect the distinct image references of those resources' containers and begin a pull for each one not present on the host.
For each reference collected, the runtime MUST record a [pin](#pin) for the calling app, creating it if absent.

The runtime MUST record each `rt.warm_images` call in the [action execution log](rule:r/history.action-log).
A repeated call with the same selection MUST observe the images on the host and return at once when every selected image is present.

## Pins

A pin is keyed by app and image reference, and MAY carry an expiry time.
While a pin exists, the runtime MUST NOT autonomously remove an image that the pin's reference resolves to.

When the runtime observes a running container whose image reference matches a pin's, it MUST delete that pin, whichever app the container belongs to.
The runtime MUST NOT create a pin again when a workload stops using an image; only another `rt.warm_images` call pins it.
An operator MAY delete any pin through the [Operator Interface](../overview.md#operator-interface).

Recording a pin that already exists MUST clear its expiry.

## Pin expiry

When a pin's expiry time has passed, the runtime MUST delete the pin on a later iteration of the reconciliation loop.
A pin whose expiry has passed but which the runtime has not yet deleted MUST still protect its image.

Only [pin reconciliation](#pin-reconciliation) sets an expiry.

## Pin reconciliation

When an operator changes an app's definition or parameter values, and the app's [resource graph](../overview.md#resource-graph) is evaluated again as a result, the runtime MUST reconcile the app's pins against the new definition.
The runtime MUST NOT reconcile pins on an iteration of the reconciliation loop that no operator change caused.

The runtime MUST run a lenient [probe](#image-discovery) of the new definition, then build the app's safe set: every image reference a Deployment or Job in the new resource graph declares, together with every image reference the probe reports.
The runtime MUST then treat each of the app's pins as follows:

| pin's reference | probe outcome | effect |
| --- | --- | --- |
| in the safe set | any | the pin's expiry is cleared |
| not in the safe set | every handler probed without error and none skipped | the pin is deleted |
| not in the safe set | any handler errored or was skipped | the pin's expiry is set to 30 days from now, unless an earlier expiry is already set |

If the probe cannot run, for instance because the definition does not compile, the runtime MUST leave the app's pins unchanged.

## Image usage tracking

The runtime MUST keep, for each image present on the host, a last-used time, set when the image is first observed on the host.
On each iteration of the reconciliation loop, the runtime MUST set the last-used time of every image that a running container uses to the current time.

## Image garbage collection

The runtime MUST periodically remove images from the host that meet every condition below:

- no pin's reference resolves to the image;
- no running container uses the image;
- the image's last-used time is at least 30 days in the past.

The runtime SHOULD do this once an hour.
Every running container on the host, whether a workload container, an infrastructure container, a shell, or a container the runtime did not start, MUST count as using its image.

If the runtime cannot determine whether an image is pinned or in use, it MUST NOT remove the image in that pass.
A failure to remove one image MUST be logged, and MUST NOT prevent the removal of other images in the same pass.

The runtime MUST NOT autonomously remove an image last used fewer than 30 days ago.
Operators remove images sooner through the Operator Interface.

## Image discovery

The runtime MUST provide a probe that evaluates an app's install handler, start handler, action handlers, shell handlers and parameter-change handlers, each with the app's current resource graph and a resolved parameter map.
A probe MUST NOT change the host, the app's persisted state, the [action execution log](rule:r/history.action-log), the [autonomous operations log](rule:r/history.operations), the [world observation history](rule:r/history.world), or any pin.

During a probe, the runtime MUST handle `rt.*` calls as follows:

| call | effect during a probe |
| --- | --- |
| `rt.start` | records the image references of the resources' containers, and schedules nothing |
| `rt.warm_images` | records the image references of the resources' containers, and writes no pin |
| `rt.exec` | does nothing, and reports exit code 0 |
| `rt.stop`, `rt.query`, `rt.restart`, `rt.warm_certs`, `rt.signal`, `rt.write` | does nothing |

During a probe, every barrier MUST resolve as satisfied at once.
This includes `.scheduled()`, `.running()`, `.ready()`, `.ready_eventually()`, `.terminated()`, `.terminated_eventually()`, and the implicit barrier of `rt.stop`.
`Termination::ensure_success()` MUST succeed.

If a handler's closure throws, the probe MUST report the thrown message as that handler's error, together with the image references recorded before the throw.

> [!NOTE]
> A probe follows the success path of every closure, so an image reference reached only after a failure, such as in a branch taken when `ensure_success()` fails, is not reported.

### Probe parameters

A probe caller MAY supply parameter values for each handler.
The runtime MUST resolve each of a handler's parameters from the first of these sources that has a value:

1. the value the probe caller supplied;
2. the app's persisted parameter value;
3. the parameter's declared default.

A parameter with no value from any source is unresolved.

A probe runs in strict or lenient mode.
If a handler declares a required parameter that is unresolved, the runtime MUST NOT evaluate the handler's closure.
In strict mode, the probe MUST report an error for that handler naming the unresolved parameters.
In lenient mode, the probe MUST report the handler as skipped, with the same detail, and report no images for it.
