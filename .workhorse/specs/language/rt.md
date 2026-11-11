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

A method of `Started` that waits for that `Started`'s resources to reach a [lifecycle state](../overview.md#lifecycle-state).

## Handle availability

The runtime MUST pass a runtime handle as the first argument of every action closure.
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

`rt.warm_certs(selection)` MUST start provisioning a certificate for every TLS-terminating [ingress](ingress.md#declaration) of the selection, and returns a `Started` over those ingresses.
It MUST ignore every other resource of the selection.
It MUST NOT route traffic to an ingress.

On a `Started` returned by `rt.warm_certs`, `ready()` and `ready_eventually()` MUST wait until every ingress it holds has a valid certificate, instead of waiting for the Ready state.
Such a `Started` holding no ingress MUST satisfy them at once.

> [!NOTE]
> In a [change handler](parameter.md#change-handlers), `app` is the app at the new generation, so `rt.warm_certs(app)` warms the ingresses of the new generation and none that only the previous one declared.

### Image warming

`rt.warm_images(selection)` MUST make present in the host's local image storage the image of every [deployment](deployment.md#declaration) and [job](job.md#instances) of the selection that declares one, without starting any container, and returns a `Started` over those resources.
It MUST ignore every other resource of the selection.
The runtime MUST [pin](../runtime/images.md#pins) each distinct image reference warmed to the app.

On a `Started` returned by `rt.warm_images`, `ready()` and `ready_eventually()` MUST wait until every image it covers is present locally, instead of waiting for the Ready state.
Such a `Started` covering no image MUST satisfy them at once.

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
Each name MUST match `[A-Za-z_][A-Za-z0-9_]*`.
An `options` map carrying any other key, an `env` that is not a map, an invalid name, or a value that is not a string MUST make `rt.exec` throw.

The runtime MUST NOT return the command's standard output or standard error to the script.
The runtime SHOULD record them with the target container's logs.
Each call MUST run its command at most once, including when the action closure is replayed, and a replayed call MUST return the exit status recorded when it ran.
`rt.exec` called while no action closure is running MUST throw.

### Volume writes

`rt.write(target, path: string, contents: string)` and `rt.write(target, path: string, contents: File)` write a file into a volume, and return no value.
A string `contents` MUST be written as its UTF-8 bytes, and a [`File`](bundle.md#bundle-files) byte for byte as the bundle holds it.

`target` is one of:

- a named Volume;
- an anonymous Volume created in the running operation;
- an ExternalVolume bound for the running operation, as [operation-scoped external volumes](volume.md#external-volumes) specify.

Any other `target`, including an ExternalVolume with no binding for the operation, MUST make `rt.write` throw.
`path` MUST be validated as a [volume write path](volume.md#write-paths) is, and an invalid `path` MUST make `rt.write` throw.

The runtime MUST NOT write the file again when a container using the volume restarts.
Each call MUST write at most once, including when the action closure is replayed.
`rt.write` called while no action closure is running MUST throw.

> [!NOTE]
> A file written into a tmpfs volume is gone the next time a container using the volume starts.

## Waiting on resource state

### Started

A `Started` is an opaque value standing for a group of resources a runtime handle method returned.
A `Started` MUST offer the [Collection](collection.md#collection) methods `one`, `only`, `except` and `select`, each returning a `Started` over the resources it selects instead of the resources themselves.
`one()` on a `Started` holding no resources MUST return unit.
A `Started` returned by a Collection method MUST keep the barriers of the `Started` it came from.

### Barriers

`Started` has these barriers, each waiting until every resource it holds has reached the lifecycle state named, or a [later one](../runtime/lifecycle.md#transitions):

| barrier | state | default deadline in seconds | returns |
| --- | --- | --- | --- |
| `scheduled()`, `scheduled(deadline)` | Scheduled | 30 | the `Started` |
| `running()`, `running(deadline)` | Running | 30 | the `Started` |
| `ready()`, `ready(deadline)` | Ready | 30 | the `Started` |
| `terminated()`, `terminated(deadline)` | Terminated | 21600 | a [`Termination`](#termination) |
| `ready_eventually()` | Ready | none | the `Started` |
| `terminated_eventually()` | Terminated | none | a `Termination` |

The default deadline of `rt.stop` is 30 seconds.

`deadline` is an integer number of seconds.
A positive `deadline` MUST be used as given, zero MUST select the default, and a negative `deadline` MUST make the barrier throw.

A barrier on a `Started` holding no resources MUST return at once.
A barrier whose condition holds MUST return, even if its deadline has passed.
If the deadline passes while the condition does not hold, the barrier MUST throw.
A barrier with no deadline MUST wait until its condition holds or the operation is [cancelled](../runtime/operations.md#cancellation).
While a barrier waits, the action closure is suspended as [suspension](../runtime/suspension.md#barriers) specifies.

### Termination

A `Termination` is an opaque value standing for how the resources of a `Started` ended.

`termination.ensure_success()` returns no value, and MUST throw unless every resource the `Termination` covers succeeded.
The error SHOULD name the resources that did not succeed.

A deployment or job instance succeeded if its most recent run exited with status 0.
If the runtime did not observe that run's exit status, the run failed if the host's process supervisor reported it as failed, and succeeded otherwise once the run has ended.
Any other resource succeeded once it reached Terminated.
A resource whose outcome the runtime cannot determine MUST count as not having succeeded.

### Executed

An `Executed` is an opaque value standing for a command `rt.exec` ran.

| method | returns |
| --- | --- |
| `exit_code()` | the command's exit status, an integer |
| `success()` | `true` if the exit status is 0, `false` otherwise |
| `ensure_success()` | no value |

An exit status MUST be the command's exit code, from 0 to 255, or 128 plus the signal number for a command a signal ended.
`ensure_success()` MUST throw if the exit status is not 0.
Its error MUST carry the exit status, and SHOULD name the container instance the command ran in.
