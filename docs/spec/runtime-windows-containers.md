The Seedling Windows Container Runtime is an implementation of the Seedling runtime for Windows Server hosts. It runs each workload as a process-isolated Windows container, composed at image-preparation time from a single OS base and the workload's artifact. It conforms to the operator interface spec and the language spec, and to the portable runtime spec (reconciliation, generations, lifecycle operations, barriers, history, faults, scheduling); this document defines the Windows infrastructure those portable semantics run on. Rule IDs use the `wcr[...]` namespace.

# Platform

> wcr[platform.floor]
> The runtime runs on Windows Server hosts providing the Host Compute Service and Host Networking Service. Workloads run as process-isolated containers sharing the host kernel.

# Base Image and Composition

> wcr[base.image]
> The runtime uses a single OS base image for all workloads, pulled from Microsoft's container registry and cached in the image store. The base pulled matches the host's OS build, so process isolation applies. When a host update changes the build, the runtime pulls the matching base for the new build.

> wcr[compose.chain]
> When an app image is prepared into the store, the runtime composes a runnable layer chain by stacking the [base](#wcr--base.image) layers beneath the artifact's layers. Starting an instance stacks a discardable scratch layer over the chain; the composed chain is shared across every instance and generation of that image.

# Containers

> wcr[engine.lifecycle]
> The runtime runs each instance as a process-isolated container through containerd and its runhcs shim. containerd is a runtime-managed infrastructure dependency: seedlingd starts it ahead of the workloads and infrastructure that need it, and stops it once no workload remains, so an idle host runs only seedlingd.

> wcr[container.model]
> Each instance runs as one process-isolated container enclosing the workload's process tree, its [network compartment](#wcr--net.compartment), its mapped volumes, and its scratch layer. Stopping the container stops everything within it.

> wcr[shim.ownership]
> Each container is supervised by its runhcs shim, which owns the container and restarts the workload per policy on exit. The shim runs independently of containerd and seedlingd: a workload keeps running while either restarts, and a restarting containerd re-attaches to its shims. A shim's death stops its container, which seedlingd reconciles as an observed exit.

> wcr[daemon.reconnect]
> On restart, seedlingd reconnects to containerd and folds the container state and the exit events it reports into the observation history.

# Networking

> wcr[net.compartment]
> Each instance's container has its own network compartment on the Seedling network, satisfying `r[infra.pod.network]`. The workload binds its listeners on all interfaces within its compartment, as a container conventionally does.

> wcr[net.dataplane]
> Service-address and mount-graph reachability (`r[infra.dataplane.service-dnat]`, `r[infra.dataplane.mount-dnat]`, `r[infra.dataplane.forward-policy]`) is realised at the compartment boundary: an instance's service address routes to its container's endpoint, a mount from A to B admits traffic from A's compartment to B's service address, and traffic outside the compiled mount graph is refused. Because the policy is attached to the compartment from the host side, the workload cannot alter its own reachability. The service address routes to the ready backing instance, so a replacement generation receives traffic once ready (`r[update.rolling]`, `r[autonomous.healthcheck-replace]`).

> wcr[net.dns]
> Each compartment resolves the Seedling zone through the [resolver](#wcr--infra.resolver) (`r[infra.pod.dns]`); other resolution follows the host's configuration.

# Infrastructure Services

> wcr[infra.services]
> The ingress controller and the resolver run as runtime-managed containers on the Seedling network. The runtime renders their configuration from desired state, starts them in dependency order ahead of the workloads that need them, and stops them once no workload requires them, so a host with no workloads runs no infrastructure containers.

> wcr[infra.ingress]
> The ingress container binds the host's configured public ports and dials backends on their service addresses (`r[infra.proxy.startup]`, `r[lifecycle.ingress]`). Configuration changes apply by graceful reload, so established connections are not dropped.

> wcr[infra.resolver]
> The resolver container serves the Seedling zone on its service address (`r[infra.resolver]`); seedlingd renders its zone data and upstream forwarding.

# Volumes

> wcr[volume.model]
> A volume is a runtime-owned host directory mapped into the consuming instance's container at a rendered path, read-only or read-write per the consuming declaration. An instance reaches only the volumes mapped into its own container.

# Shutdown and Signals

> wcr[stop.methods]
> A workload's [process profile](#wcr--artifact.profile) declares how it is asked to stop, one of:
>
> - `ctrl_break` / `ctrl_c`: the shim delivers a console control event to the workload's process group.
> - `named_event`: the shim passes an event name in the environment and signals that event; a sibling reload event may be declared for reload.
> - `terminate`: the container is terminated directly.

> wcr[stop.ladder]
> The stop sequence delivers the declared [stop method](#wcr--stop.methods), waits `stop_timeout` (`l[container.stop-signal]`, `l[container.stop-timeout]`), then terminates the container.

> wcr[signal.map]
> `rt.signal(target, name)` maps POSIX signal names onto Windows mechanisms:
>
> - `SIGTERM`, `SIGINT`, `SIGQUIT`, `SIGKILL` terminate the target's container, leaving desired state unchanged; the reconciler sees an exit and restarts per desired state.
> - `SIGHUP` signals the target profile's reload event where one is declared, and is recorded as skipped where none is.
> - Other signal names are recorded as skipped (`r[rt.signal]`).

> wcr[signal.exit-code]
> When the runtime terminates a process — the [stop ladder](#wcr--stop.ladder)'s final step, a signal-mapped termination, or a session teardown — it records a negative exit code, so `i[shell.exit]`'s convention distinguishes runtime termination from the process's own exit code.

# Capabilities

> wcr[capability.map]
> The runtime reports `storage:block-clone` true when the volume root is ReFS. Snapshot and NAT64 capabilities are reported absent.

# Actions and Shells

> wcr[action.exec]
> An `Executed` command runs as a new process inside the target instance's container, under the workload's account, environment, and working directory. It shares the container's lifetime: stopping the instance ends the command.

> wcr[shell.session]
> A shell session runs a process inside the target's container under a ConPTY pseudoconsole (`i[stream.shell]`): operator input drives the console input, console output drives the session's output stream, and resize requests resize the console.

> wcr[shell.volume]
> A volume shell runs a container with the selected volumes mapped in, named by display name, launched with that directory as its working directory (`i[volumes.shell]`). Read-only and read-write sessions differ only in how the volumes are mapped.

# Artifacts

> wcr[artifact.format]
> A Windows workload is delivered as an OCI image built without a base image: its layers carry only the workload's own filesystem, and its config declares the entrypoint, command, environment, working directory, exposed ports, and [process profile](#wcr--artifact.profile). It is an ordinary OCI image — content-addressed layers, standard manifest and config — produced, stored, signed, and replicated with standard registry tooling, and completed for execution by [composition](#wcr--compose.chain).

> wcr[artifact.profile]
> The config's process-profile fields declare the workload's [stop method](#wcr--stop.methods) and whether it supports a reload event. A BSL deployment may override these per the language spec's stop-configuration surface.

> wcr[artifact.readonly]
> The artifact's layers are read-only: [composition](#wcr--compose.chain) stacks them beneath a discardable scratch layer, so per-instance writes are ephemeral and durable state lives in [volumes](#wcr--volume.model).
