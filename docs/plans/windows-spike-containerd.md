# Spike: containerd survival + control (the decider)

The load-bearing spike for the container runtime. Its outcome chooses between
building on containerd (option B) and hand-rolling the compute plane on the
Compute\* APIs (option A). Everything else in the design composes with whatever
this spike settles.

Environment: a Windows Server 2019+ host with the Containers feature,
containerd and `ctr` installed, and a base image whose build matches the host
(`wcr[base.image]`). Run elevated — stopping and starting the containerd service
requires it.

## At stake

- `wcr[shim.ownership]` — the runhcs shim owns the container and outlives
  containerd, so the workload keeps running while containerd is stopped.
- `wcr[daemon.reconnect]` — a restarting containerd re-attaches to the
  surviving shim and resumes reporting task state.
- `wcr[engine.lifecycle]` — containerd can be started on demand and stopped
  when the world is empty, without disturbing the content store.
- `wcr[compose.chain]` — the runtime's base stacks beneath a base-less
  artifact's layers into a runnable chain (validated in depth by the
  composition spike; touched here only enough to get a container running).

## Experiments

1. **Preflight.** Confirm the containerd service is registered and `ctr`
   responds; record the host OS build so the base tag can be matched
   (`wcr[base.image]`). A missing base for the current build is the expected
   first failure on a freshly patched host, and is an operational condition,
   not a spike failure.
2. **Start a long-lived container.** Pull the base, run a detached
   process-isolated container with a keep-alive process. Record the workload
   task PID and confirm a `containerd-shim-runhcs-v1` process is present.
   Process isolation is the default when the base matches the host; Hyper-V
   isolation would be an explicit option and is out of scope.
3. **Stop containerd; confirm the workload survives (make-or-break).** Stop
   the containerd service. Confirm the workload's PID is still alive — checked
   directly by opening the process, which does not depend on containerd — while
   `ctr` can no longer reach the daemon. This is the property the whole design
   rests on (`wcr[shim.ownership]`).
4. **Restart containerd; confirm re-attach.** Start the service again and list
   tasks. Expect the same task reported RUNNING with its original PID:
   containerd re-attached to the surviving shim rather than losing or
   duplicating it (`wcr[daemon.reconnect]`).
5. **On-demand teardown.** Kill and remove the container, then stop the
   containerd service with the world empty. Confirm no containers remain and
   the content store and pulled images are intact for the next start
   (`wcr[engine.lifecycle]`).
6. **Control path (follow-up).** Repeat the create/start/stop/exec/observe
   cycle through the gRPC control API the production daemon will use (the
   `containerd-client` crate) rather than `ctr`, to confirm the Rust client
   drives containerd over its named pipe. Separable from the survival property,
   which experiments 3–4 settle independently of the client.

## Exit criteria

- The workload PID stays alive across a full containerd stop, and the task is
  reported RUNNING with its original PID after containerd restarts. This is the
  gate: if it holds, the container runtime builds on containerd.
- On-demand stop leaves no containers and an intact content store.
- The gRPC control path drives a container lifecycle end to end (may trail the
  survival result).

## If it fails

- If the workload does not survive a containerd restart, containerd cannot be
  treated as a restartable managed dependency, and the daemon-independence
  property must be built directly: fall back to option A — seedlingd owns the
  supervisor, reattach, and layer sequencing on the Compute\* APIs
  (`computecore` / `computestorage` / `computenetwork`) through the `windows`
  crate. Use experiment 3's result to price that work before committing.
- If re-attach duplicates or orphans tasks rather than resuming cleanly, the
  gap is in the reattach path; capture the failing sequence for upstream before
  reaching for the fallback.
