---
id: RT
---

# The runtime handle

## Runtime Instance

<!-- from language: Runtime Instance -->

The Runtime Instance is a handle to the Seedling runtime for an application.
It's how the script actually controls the containers in the "outside world".

This spec defines the semantics of the Runtime Instance as far as BSL is concerned; the exact implementation and control plane semantics are defined in other places, and must not be relied upon by BSL scripts.

> l[rt.var]
> `rt` is a variable available within actions (usually as the first argument of the closure).

> l[rt.type]
> The `rt` variable is of type `RuntimeInstance`.

> l[rt.constructor]
> The `rt` type is not constructible within a BSL.

> l[rt.methods]
> All the methods of `rt` are defined in this spec.

> l[rt.lifecyle]
> - _Pending_: the resource is active in the runtime state, but not yet scheduled.
> - _Scheduled_: the resource is set up "in the world" (on the node or cluster).
> - _Running_: the resource is running, but may not yet be ready.
> - _Ready_: the resource is ready to be used.
> - _Terminating_: termination has been initiated by the runtime.
> - _Terminated_: the resource has terminated.
> - _Unscheduled_: the resource has been cleaned up.
>
> It's the runtime's concern as to how the scheduling and other lifecycle actions and events work for each resource type.
>
> Note that a resource can transition directly from _Running_ or _Ready_ to _Terminated_, for example when it exits on its own.

## Workload control

<!-- from language: Runtime Instance / Workload control -->

> l[rt.start]
> The `rt.start(resources: Collection)` method schedules the resources in the Collection. It returns a [Started](rule:l/rt.started).
>
> Action handles are not valid arguments to `rt.start`: actions are not resources and `rt.start` does not invoke their closures. Use [`Action.invoke(params?)`](rule:l/action.call) to invoke an action from a dynamic context. Passing an Action (or a Collection containing one) to `rt.start` must throw.

> l[rt.stop]
> The `rt.stop(resources: Collection, deadline?: number)` method unschedules the resources in the Collection and blocks until all terminate.
> `deadline` is interpreted the same as for [Started](rule:l/rt.started.state-methods).

> l[rt.query]
> The `rt.query(resources: Collection)` method returns a [Started](rule:l/rt.started) _without_ scheduling the resources.

> l[rt.restart]
> The `rt.restart(deployment: Deployment)` method triggers a restart of all running instances of the named Deployment, following its configured update strategy ([on_update](rule:l/deployment.on-update)).
> It does not change the deployment's definition or generation, and returns no value.
> Triggering semantics and durability are specified in [deployment.restart](rule:i/deployment.restart) at the interface level.

> l[rt.warm-certs]
> The `rt.warm_certs(resources: Collection)` method selects all TLS-terminating [Ingresses](rule:l/ingress.type) from the given Collection and ensures their TLS certificates are provisioned and cached, without yet routing traffic to those ingresses.
> Non-ingress resources and ingresses without TLS termination in the collection are ignored.
>
> It returns a [Started](rule:l/rt.started). Calling `.ready()` on the returned `Started` blocks until certificates are valid for every selected ingress.
> If certificate provisioning cannot complete for one or more selected ingresses (for example, ACME issuance fails persistently), a fault is filed and the barrier throws when its deadline expires.
>
> If the selection contains no TLS-terminating ingresses, the returned `Started` is immediately satisfied.
>
> Calling `rt.warm_certs(app)` from within an [`on_change`](rule:l/param.on-change) handler warms exactly the ingresses that exist in the new generation. Ingresses present in the previous generation but not the new one are not warmed.

> l[rt.warm-images]
> The `rt.warm_images(resources: Collection)` method selects all [Deployment](rule:l/deployment.type) and [Job](rule:l/job.type) resources from the given Collection, extracts their container image references, and ensures those images are present in local container storage, without starting the containers.
> Non-container resources, and container resources that have no image reference declared, are ignored.
>
> Each distinct image reference in the selection is _pinned_ to the calling app: a pinned image is protected from autonomous image removal until a running container is observed using it (at which point the pin is cleared automatically) or an operator clears the pin explicitly. Pins are not automatically re-established when a workload that was using an image stops; a subsequent `rt.warm_images` call is required to re-pin.
>
> It returns a [Started](rule:l/rt.started). Calling `.ready()` on the returned `Started` blocks until every selected image is present locally.
> If an image pull fails persistently, a fault is filed and the barrier throws when its deadline expires.
>
> If the selection contains no images to warm, the returned `Started` is immediately satisfied.
>
> Anonymous container resources are supported: `rt.warm_images(app.job().image("registry/foo:1.2.3"))` warms the single image reference of a dynamic job without ever starting it, which is the idiomatic way to pre-load a future version of an image ahead of a deploy.

> l[rt.signal]
> The `rt.signal(target: Collection, signal: string)` method delivers the named POSIX signal to PID 1 of every running container instance in the selected Collection.
>
> The `signal` argument accepts the canonical `"SIGFOO"` form or its bare `"FOO"` shorthand; unknown signal names are a script-evaluation error.
>
> Selection rules:
>
> - Container instances that are not running are silently skipped (no error). This makes `rt.signal` safe to use against deployments where some replicas may be in transient states.
> - Non-container resources in the collection are ignored.
> - An empty selection is an error — typically a sign that the caller's collection expression resolved to nothing.
>
> The call is _at-most-once_ across replays: when the runtime restarts mid-operation and replays the action closure, a previously-delivered signal is not re-sent.
>
> Typical use cases: `SIGHUP` to reload configuration without restart (postgres, nginx), `SIGUSR1` to trigger log rotation, `SIGTERM`/`SIGINT` for cooperative shutdown when the deployment's `stop_signal` is not what the action wants for this one-off invocation.

> l[rt.exec]
> The `rt.exec(target, args: array<string>)` and `rt.exec(target, args: array<string>, options: map)` methods run a command inside an existing running container.
>
> The `target` argument is one of:
>
> - a `Started` handle (typically the return value of `rt.start(...)` earlier in the same action),
> - a named `Deployment` (the runtime resolves it to its currently-running instance),
> - a named `Job` (the runtime resolves it to its currently-running instance).
>
> The target must resolve to exactly one running container; otherwise calling `rt.exec` is a script error. Jobs and Deployments with the potential for more than one instance (e.g. `Deployment.scale(2)` or `Deployment.scale(1..3)`) are rejected without consulting the registry — the upper bound of the scale range must be `1`.
>
> `args` is the command and its arguments (argv). It must be a non-empty array of strings.
>
> The optional `options` map currently supports:
>
> - `env: #{ KEY: "value", ... }` — extra environment variables for the command, layered on top of the container's environment. Keys must be valid environment-variable names (POSIX `[A-Za-z_][A-Za-z0-9_]*`).
>
> Other option keys are reserved for future extension and currently rejected.
>
> `rt.exec` does NOT capture stdout or stderr; both streams are forwarded to the runtime's container-log sink for the target container.
>
> The call is _at-most-once_ across replays: when the runtime restarts mid-operation and replays the action closure, a previously-completed `rt.exec` is not re-executed; the recorded exit code is recovered from the action log.
>
> Calling `rt.exec` outside an action closure is a script error.

> l[rt.executed.type]
> `Executed` is an opaque type returned by `rt.exec`. It carries the exit code of the command and offers methods to inspect it.
>
> Discarding the returned `Executed` without calling any of its methods is supported and is the natural way to "fire and forget" a command whose result the script does not care about.

> l[rt.executed.exit-code]
> `Executed.exit_code()` returns the integer exit code of the command. A non-zero value indicates the command failed; the exact mapping (0..255 for normal exits, larger values for signal-terminated processes) follows the host convention.

> l[rt.executed.success]
> `Executed.success()` returns `true` when the command exited with code `0`, `false` otherwise. Equivalent to `executed.exit_code() == 0`.

> l[rt.executed.ensure-success]
> `Executed.ensure_success()` throws a script error when the command exited non-zero. The error message includes the exit code. It is the most concise way to assert that a command succeeded; the matching pattern is `rt.exec(...).ensure_success()`.

> l[rt.write]
> The `rt.write(target: Volume | ExternalVolume, path: string, contents: string | File)` method writes a file into the given volume at action runtime, parallel to the static `Volume.write`.
> A [`File`](rule:l/file.type) is written byte for byte as it is held in the bundle.
>
> The target may be:
>
> - a named (static) `Volume` declared at the top level,
> - an anonymous `Volume` created earlier in the same action closure, or
> - an `ExternalVolume`, including those resolved from operation-scoped volume bindings (see `l[action.params.volume]`).
>
> The path must be absolute, must not escape the volume root after canonicalisation, and must not resolve to the volume root; these are the same validation rules as `l[volume.write.validation]`.
>
> Unlike static `Volume.write`, `rt.write` does NOT reapply on container restart. It is a point-in-time write at action time. For tmpfs volumes that means the contents are erased on the next container start; this is allowed and is the user's responsibility to reason about.
>
> The call is _at-most-once_ across replays: when the runtime restarts mid-operation and replays the action closure, a previously-completed `rt.write` is not re-executed.
>
> Calling `rt.write` outside an action closure is a script error.

## Waiting on resource state

<!-- from language: Runtime Instance / Waiting on resource state -->

> l[rt.started.type]
> `Started` is an opaque type representing some resources that have been started.
>
> `Started` implements [Collection](rule:l/collection.interface), except that all the resources returned by Collection's methods return `Started`s corresponding to the resources, instead of the original resources.

> l[rt.started.state-methods]
> `Started` has a number of methods of the form `started.<state>(deadline?: number)` which block until all resources have entered the state `<state>` (one of `scheduled`, `running`, `ready`, `terminated`).
>
> The argument `deadline` must be a positive integer number of seconds; if it's zero or absent, the default deadline for that state is used (see [default deadlines](rule:l/rt.started.default-deadlines)).
>
> If the deadline is reached before the method returns, an exception is thrown.
>
> A `Started` holding no resources has nothing to wait on, so these methods return at once.
> This is how a group narrowed by a [Collection](rule:l/collection.interface) method down to nothing behaves, and narrowing to nothing is an ordinary outcome of `except` or `select`: a barrier that instead waited forever would hang an action over a selection the author wrote deliberately.

> l[rt.started.default-deadlines]
> The default deadlines for `started.<state>()` barriers with no explicit `deadline` argument are:
>
> - `scheduled`, `running`, `ready`: short (on the order of tens of seconds), because these barriers guard correctness signals — a resource that has not reached these states in that window usually indicates a cluster-level problem rather than a slow workload.
> - `terminated`: long (on the order of hours), because `terminated` is routinely called on Jobs that run for extended periods.
>
> Every default deadline is a positive non-zero number of seconds. The exact values are set by the control plane and are not specified here.
>
> Callers with an unbounded wait requirement use [`started.terminated_eventually()`](rule:l/rt.started.terminated-eventually) or [`started.ready_eventually()`](rule:l/rt.started.ready-eventually) instead of passing a very large deadline.

> l[rt.started.terminated]
> The `started.terminated()` state method returns a [Termination](rule:l/rt.termination.type).

> l[rt.started.terminated-eventually]
> `started.terminated_eventually()` behaves like [`started.terminated()`](rule:l/rt.started.terminated), but has no deadline: it blocks until all resources reach `terminated` or the operation is [cancelled](rule:r/operation.cancel).
>
> Use this for jobs with genuinely unbounded duration (e.g. large backups, bulk restores) where any finite deadline is either too short to be safe or too long to be useful as a failure signal.

> l[rt.started.ready-eventually]
> `started.ready_eventually()` behaves like `started.ready()`, but has no deadline: it blocks until all resources reach `ready` or the operation is cancelled.
>
> Typical use: ingress cert provisioning via an external CA (e.g. Let's Encrypt) whose completion time is bounded only by the CA's rate limiting.

> l[rt.termination.type]
> `Termination` is an opaque type representing the termination state of a resource.

> l[rt.termination.ensure-success]
> The `termination.ensure_success()` method throws if the resource terminated without succeeding.
>
> For a container-backed resource (Deployment, Job), "succeeded" means the container's process exited with status 0. Any non-zero exit status — including termination by signal — is considered a failure. When the runtime observes the container's exit code directly, that observation is authoritative. When the exit code is unobservable (for example because the container was auto-removed before the runtime could inspect it), the runtime may fall back to secondary signals: a systemd-level unit failure counts as a failure; otherwise the run is treated as a success provided the instance reached a terminal lifecycle state.
>
> When a [Started](rule:l/rt.started.type) group holds multiple resources, all of them must have succeeded; otherwise `ensure_success()` throws.
>
> For other resource kinds with no natural exit code (Service, Ingress, Volume), termination itself is success.

<!-- TODO: more Termination methods -->
