---
id: RT
---

# The runtime handle

Applies to the runtime handle through which an [action closure](action.md#action-closure) acts on its [app](../overview.md#app)'s resources, and to the values its methods return.

## Terminology

### Runtime handle

The value, of type `RuntimeInstance`, through which an action closure directs the [runtime](../overview.md#runtime) to act on its app's [resources](../overview.md#resource).

### Selection

The argument naming the resources a runtime handle method acts on.

### Barrier

A call in an action closure that waits for resources to reach a [lifecycle state](../overview.md#lifecycle-state): a state method of a [`Started`](#started), or [`rt.stop`](#stopping-resources).

## Handle availability

The runtime MUST pass a runtime handle as the first argument of every action closure, except a [change handler](parameter.md#change-handlers) that takes no arguments.
Scripts conventionally name it `rt`.
The runtime MUST NOT expose any way for a script to construct a `RuntimeInstance`.

## Selections

A selection is one of these values, holding the resources listed:

| value | resources |
| --- | --- |
| an `App` | the app's named resources |
| a [Collection](collection.md#collection) | the Collection's resources |
| a `Started` | the resources the `Started` holds |
| a resource handle, named or anonymous | that resource |
| an array of selections | every resource of every element |

A runtime handle method MUST act only on the deployments, jobs, services and ingresses of a selection, and MUST ignore its other resources.
A selection that is an `Action`, or any value not in the table, MUST make the method throw.

A runtime handle method MUST resolve each resource of a selection to its instances when the method is called.
A named Deployment MUST resolve to as many instances as its current instance count under its [scale](deployment.md#scale), and to at least one where that count is 0.
Every other resource MUST resolve to its one instance.

## Workload control

### Starting resources

`rt.start(selection)` MUST place every resource of the selection in the app's [desired state](../overview.md#desired-state), and returns a [`Started`](#started) over those resources.

### Stopping resources

`rt.stop(selection)` and `rt.stop(selection, deadline: number)` MUST remove every resource of the selection from the app's desired state, then wait until all of them have reached Terminated, and return no value.
`deadline` is read as for [barriers](#barriers), with the default given there for `rt.stop`.
If the deadline passes first, `rt.stop` MUST throw.
While it waits, the action closure is suspended as [suspension](../runtime/suspension.md#rtstop) specifies.

### Querying resources

`rt.query(selection)` returns a `Started` over the selection's resources, and MUST NOT change the desired state.

### Deployment restart

`rt.restart(deployment: Deployment)` MUST restart every running instance of the named deployment, following its [update strategy](deployment.md#update-and-termination-strategies), and returns no value.
It MUST NOT change the deployment's definition or the app's [generation](../overview.md#generation).
An anonymous deployment, or any value other than a Deployment, MUST make `rt.restart` throw.
Each call MUST restart the deployment at most once, including when the action closure is [replayed](../runtime/suspension.md#replay).
Restart semantics are specified by [deployment restart](../interface/app-control.md#deployment-restart).

### Certificate warming

`rt.warm_certs(selection)` MUST start provisioning a certificate for every [TLS-terminating ingress](ingress.md#tls-terminating-ingress) of the selection, and returns a `Started` over those ingresses.
It MUST ignore every other resource of the selection.
It MUST NOT route traffic to an ingress.
A certificate acquired this way MUST be the one served when the same ingress is later started.
`rt.warm_certs` MUST NOT begin acquisition again for an ingress whose hostname already has a usable certificate [served](../runtime/tls/certificates.md#served-certificate).

On a `Started` returned by `rt.warm_certs`, `ready()` and `ready_eventually()` MUST wait until a usable certificate is served for the hostname of every ingress it holds, instead of waiting for the Ready state.
Such a `Started` holding no ingress MUST satisfy them at once.
Its other barriers MUST throw.

> [!NOTE]
> In a [change handler](parameter.md#change-handlers), `app` is the app at the new generation, so `rt.warm_certs(app)` warms the ingresses of the new generation and none that only the previous one declared.

### Image warming

`rt.warm_images(selection)` MUST make present in the host's local image storage the image of every [deployment](deployment.md#declaration) and [job](job.md#instances) of the selection that declares one, without starting any container, and returns a `Started` over those resources.
It MUST ignore every other resource of the selection.
The runtime MUST [pin](../runtime/images.md#pins) each distinct image reference warmed to the app.

On a `Started` returned by `rt.warm_images`, `ready()` and `ready_eventually()` MUST wait until every image it covers is present locally, instead of waiting for the Ready state.
Such a `Started` covering no image MUST satisfy them at once.
Its other barriers MUST throw.

> [!NOTE]
> A selection can hold an anonymous job that is never started: `rt.warm_images(app.job().image("registry/foo:1.2.3"))` pre-loads an image ahead of the deploy that uses it.

### Signal delivery

`rt.signal(selection, signal: string)` MUST deliver the named signal to the main process of every running container instance of every deployment and job of the selection, including every replica, and returns no value.
`signal` MUST be accepted in the forms a [stop signal](container.md#stopping) accepts, and an unrecognised name MUST make `rt.signal` throw.
The runtime MUST skip container instances that are not running.
A selection holding no deployment or job MUST make `rt.signal` throw.
If delivery to a running instance fails, `rt.signal` MUST throw.
Each call MUST deliver its signal at most once, including when the action closure is replayed.

### Command execution

`rt.exec(target, args: array)` and `rt.exec(target, args: array, options: map)` run a command inside a running container, wait for it to exit, and return an [`Executed`](#executed).

`target` is one of:

- a `Started` holding exactly one deployment or job instance;
- a named Deployment whose declared scale has an upper bound of 1;
- a named Job.

Any other `target` MUST make `rt.exec` throw.
A Deployment whose declared scale has an upper bound above 1 MUST make `rt.exec` throw, whatever number of instances exist.
A `target` that does not resolve to exactly one running container MUST make `rt.exec` throw.

`args` is the command and its arguments, and MUST be a non-empty array of strings.
Any other `args` MUST make `rt.exec` throw.

`options` MAY carry `env`, a map from environment variable names to string values, added to the container's environment for the command.
Each name and value MUST meet the rules a [container environment](container.md#environment) variable meets.
An `options` map carrying any other key, an `env` that is not a map, a name or value breaking those rules, or a value that is not a string MUST make `rt.exec` throw.

The runtime MUST NOT return the command's standard output or standard error to the script.
The runtime MUST record them with the target container's logs.
Each call MUST run its command at most once, including when the action closure is replayed, and a replayed call MUST return the exit status recorded when it ran.
`rt.exec` MUST throw when called outside the [action context](app.md#action-context).

### Volume writes

`rt.write(target, path: string, contents: string)` and `rt.write(target, path: string, contents: File)` write a file into a volume, and return no value.
A string `contents` MUST be written as its UTF-8 bytes, and a [`File`](bundle.md#bundle-files) byte for byte as the bundle holds it.

`target` is one of:

- a named Volume;
- an anonymous Volume created in the running operation;
- an ExternalVolume that [resolves](volume.md#external-volumes) to storage, through the running operation's bindings or the host's mappings.

Any other `target` MUST make `rt.write` throw, including an ExternalVolume that resolves to nothing, and one whose mapping is read-only.
`path` MUST be validated as a [volume write path](volume.md#write-paths) is, and an invalid `path` MUST make `rt.write` throw.

The runtime MUST NOT write the file again when a container using the volume restarts.
Each call MUST write at most once, including when the action closure is replayed.
`rt.write` MUST throw when called outside the action context.

> [!NOTE]
> A file written into a tmpfs volume is lost when the host restarts, and the runtime does not write it again, as it does a tmpfs volume's [declared file writes](volume.md#declared-file-writes).

## Waiting on resource state

### Started

A `Started` is an opaque value standing for the instances a runtime handle method resolved its selection to.
A `Started` MUST offer the [Collection](collection.md#collection) methods `one`, `only`, `except` and `select`, each returning a `Started` over the instances it selects instead of the resources themselves.
These methods MUST judge an instance by the resource type and name of its resource.
`one()` on a `Started` holding no instances MUST return unit.
A `Started` returned by a Collection method MUST keep the barriers of the `Started` it came from.

### Barriers

`Started` has these barriers, each waiting until every instance it holds has reached the lifecycle state named or any state after it in the [transition order](../runtime/lifecycle.md#transitions):

| barrier | state | default deadline | returns |
| --- | --- | --- | --- |
| `scheduled()`, `scheduled(deadline)` | Scheduled | short | the `Started` |
| `running()`, `running(deadline)` | Running | short | the `Started` |
| `ready()`, `ready(deadline)` | Ready | short | the `Started` |
| `terminated()`, `terminated(deadline)` | Terminated | long | a [`Termination`](#termination) |
| `ready_eventually()` | Ready | none | the `Started` |
| `terminated_eventually()` | Terminated | none | a `Termination` |

The runtime sets the value of each default deadline.
A short default deadline MUST be on the order of tens of seconds.
A long default deadline MUST be on the order of hours.
Every default deadline MUST be a positive number of seconds.

The default deadline of `rt.stop` is 30 seconds more than the longest [stop timeout](container.md#stopping) among the containers of the resources it stops, or 30 seconds where it stops no container.

`deadline` is an integer number of seconds.
A positive `deadline` MUST be used as given, zero MUST select the default, and a negative `deadline` MUST make the barrier throw.

A barrier on a `Started` holding no instances MUST return at once.
A barrier whose condition holds MUST return, even if its deadline has passed.
If the deadline passes while the condition does not hold, the barrier MUST throw.
A barrier with no deadline MUST wait until its condition holds or the operation is [cancelled](../runtime/operations.md#cancellation).
While a barrier waits, the action closure is suspended as [suspension](../runtime/suspension.md#barriers) specifies.

> [!NOTE]
> An instance that has terminated satisfies `scheduled()`, `running()` and `ready()`, so those barriers do not show that an instance is still serving.
> `terminated()` and its `Termination` show whether a run succeeded.

### Termination

A `Termination` is an opaque value standing for how the instances of a `Started` ended.

`termination.ensure_success()` returns no value, and MUST throw unless every instance the `Termination` covers succeeded.

A deployment or job instance succeeded if its most recent run exited with status 0.
If the runtime did not observe that run's exit status, the run succeeded only if the host's process supervisor reported it as succeeded.
An instance of any other resource succeeded once it reached Terminated or Unscheduled.
An instance whose outcome the runtime cannot determine MUST count as not having succeeded.

### Executed

An `Executed` is an opaque value standing for a command `rt.exec` ran.

| method | returns |
| --- | --- |
| `exit_code()` | the command's exit status, an integer |
| `success()` | `true` if the exit status is 0, `false` otherwise |
| `ensure_success()` | no value |

An exit status MUST be the command's exit code, from 0 to 255, or 128 plus the signal number for a command a signal ended.
`ensure_success()` MUST throw if the exit status is not 0.
Its error MUST carry the exit status.
