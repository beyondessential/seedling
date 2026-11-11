---
id: IMG
---

# Images

## Actuation

<!-- from runtime: Actuation -->

> r[actuate.image.retry]
> A failed image pull must be retried, and successive failures for the same reference must be spaced by an increasing delay up to a bounded maximum, so that an unreachable registry is not polled at the reconciliation interval.
> A transient pull failure must not permanently disable actuation of the workload: however long a reference has been failing, waiting the maximum delay must be sufficient for another attempt.
> Past a threshold of consecutive failures the condition becomes operator-visible, but that is an escalation and not a terminal state — attempts continue at the maximum interval, which is what allows the fault to clear on a later success.
> A successful pull resets the count.

> r[actuate.image.warm]
> When an action closure invokes [`rt.warm_images`](rule:l/rt.warm-images) with a selection that contains container resources, the runtime must collect the distinct image references from those resources' container definitions and initiate pulls for each image that is not already present locally.
> For each extracted image reference the runtime must durably record a [pin](rule:r/image.pin) tying the calling app to the reference, upserted idempotently.
>
> The warm_images call must be recorded in the [action execution log](rule:r/history.action-log) and must be idempotent on replay: a subsequent invocation with the same selection observes the existing local image state and returns immediately when all selected images are present.

> r[image.pin]
> A _pin_ is a durable `(app, image_reference)` record indicating that an app has requested the referenced image be warmed.
> While a pin exists it must protect its image reference from autonomous removal (see [image.gc](rule:r/image.gc)).
> The runtime must remove a pin whenever it observes a running container whose image reference matches that pin, so that a warmed image transitions from _pinned_ to _in use_ automatically on first observed use.
> A pin must not be automatically re-created when a workload stops using an image; a subsequent `rt.warm_images` call is required to re-pin.
> Operators may clear any pin explicitly via the operator interface.

> r[image.pin.expiry]
> A pin may carry an optional expiration timestamp. When an expiration is set and passes, the reconciler must delete the pin on a subsequent tick exactly as if an operator had cleared it explicitly.
> Setting, clearing, and reading an expiration does not change any other aspect of the pin's lifecycle — in particular, an expired-but-not-yet-swept pin still protects its image from [autonomous removal](rule:r/image.gc) until the reconciler sweeps it.
> Expirations are only set by the post-update reconciliation rule (see [`image.pin.update-reconcile`](rule:r/image.pin.update-reconcile)); they are cleared whenever a pin's reference is observed to be valid again for the owning app.

> r[image.pin.update-reconcile]
> Whenever an app's `AppDef` changes because of an operator-driven edit (a script update or a parameter change that triggers re-evaluation), the runtime must re-probe the new script and use the result to reconcile that app's pins:
>
> 1. Build the _safe set_: every image reference declared by a `Deployment` or `Job` in the new `AppDef`, plus every image reference in the probe's [`all_images`](rule:r/image.discover) output.
> 2. For any pin whose reference is in the safe set, clear its expiration (if any) so the pin is kept indefinitely.
> 3. For any pin whose reference is _not_ in the safe set:
>    - If the probe ran without errors _and_ without skipped handlers, the safe set is authoritative: delete the pin.
>    - Otherwise the safe set is incomplete: set the pin's expiration to 30 days from now, unless an earlier expiration is already set.
>
> A pin's reference is "in the safe set" based on exact string equality; a tag-pinned reference and a digest-pinned reference to the same image id are treated as distinct.
> The rule fires only on operator-driven AppDef changes; autonomous reconciliation ticks do not run probes.

> r[image.track]
> The runtime must maintain, for each locally-present container image, a _last-used_ timestamp initialised to the time the image was first observed locally.
> On every reconciliation pass, the last-used timestamp of every image whose `image_id` appears on at least one running container must be updated to the current time.

> r[image.gc]
> At a regular interval matching the background garbage collector cadence, the runtime must remove locally-present container images that satisfy all of the following:
>
> - No [pin](rule:r/image.pin) exists for any reference pointing at the image.
> - No running container is currently observed using the image.
> - The image's [last-used timestamp](rule:r/image.track) is at least 30 days in the past.
>
> Removal failures must be logged and must not prevent removal of other images in the same pass.
> Autonomous image GC must not be extended with shorter retention periods or more aggressive policies without explicit operator configuration; operators remove images they no longer need through the operator interface.

> r[image.discover]
> The runtime must provide a _probe_ execution mode that runs an app's handler closures without any world-modifying side effects, in order to enumerate every container image reference the handler might pull. The probe must:
>
> - Drive every `rt.*` call whose normal semantics would mutate state or wait on the world. `rt.start(...)` and `rt.warm_images(...)` extract image references from the supplied container resources and record them to the probe output without scheduling anything or writing pins. `rt.stop(...)`, `rt.warm_certs(...)`, `rt.restart(...)`, and `rt.query(...)` are no-ops.
> - Satisfy every barrier immediately: `.scheduled()`, `.running()`, `.ready()`, `.ready_eventually()`, `.terminated()`, `.terminated_eventually()`, and `rt.stop(...)`'s implicit barrier all return as if their condition is met. `Termination::ensure_success()` succeeds unconditionally.
> - Not record anything to the action log, the autonomous operations log, the world observation history, or the `image_pins` table.
>
> The probe is invoked per handler and operates on a handler's captured closure, the current `AppDef`, and a resolved param map supplied by the caller. Error-path image references (those only reached via a closure branch predicated on `termination.ensure_success()` failing, or a similar live-state signal) are necessarily missed by the probe, since the probe always returns the success path.

> r[image.discover.params]
> The probe accepts a param map per handler. Parameter resolution proceeds in priority order:
>
> 1. Values supplied by the probe caller override everything.
> 2. Otherwise, the app's persisted param store supplies the value.
> 3. Otherwise, the param schema's `default_value` is used.
> 4. Otherwise, the param is unset.
>
> Probing may operate in either of two modes:
>
> - **Strict**: when a handler declares a required parameter that remains unresolved after step 3, the probe for that handler must return an error identifying the missing parameters without invoking the closure.
> - **Lenient**: the same condition causes the handler to be reported as _skipped_ with the same error payload, without invoking the closure or producing any images.
>
> Closures that throw during probing must be reported with the thrown message as their error; images accumulated before the throw must be returned alongside.
