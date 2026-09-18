# Windows threat model: deferred, and what it has to cover

There is deliberately no Windows threat-model document yet. A threat model written against an unbuilt runtime describes intentions rather than boundaries, and the boundaries here depend on answers the spikes have not returned. It is written once the spikes have answered and the first workload runs end to end, and before the pilot — a field host should not take traffic without one.

This records why it is deferred and what it must cover, so the reasoning accumulated while specifying the runtime is not lost between now and then.

The base threat model (`docs/threat-model.md`) applies throughout. The operator interface, BSL sandbox, secret-parameter, TLS, and audit surfaces carry over unchanged and will not need restating; what changes is everything that rested on Linux containment.

## The claims that need examining

The runtime spec makes several confinement claims. Each is either true, or true with a qualification the threat model owes the reader plainly.

**Process isolation is not a security boundary.** Microsoft's position is that process-isolated Windows containers are not a security boundary and that Hyper-V isolation is the answer where the workload is hostile. `wcr[net.dataplane]` says a workload cannot alter its own reachability, and `wcr[volume.model]` and `wcr[volume.boundary]` confine an instance to its mapped volumes; all of it rests on process isolation. This is probably an acceptable posture — the fleet's workloads are ours, not arbitrary tenants, and the Linux runtime's own containment is not a hostile-tenant boundary either — but it has to be stated, not assumed. The deleted process-native model had a rule (`win[wfp.honesty]`) that said the equivalent plainly; nothing replaces it yet.

**Volume contents are not confidential from the host.** `wcr[volume.host-exposure]` already concedes this in the spec: a mapped volume must admit an identity that does not exist on the host, so its permissions are broader than the owner-only rule applied to seedling's own state. The threat model has to say what that means for an attacker — which host principals can read workload data, and how that differs from the Linux runtime, where volumes are owner-only. The floor depends on S6 Q1: `Authenticated Users` if a host ACE cannot name a container SID, "any container process" if it can.

**Host permissions cannot separate instances** (`wcr[volume.boundary]`). Cross-workload file access is prevented by the mount graph, not by access control. That is a different shape of defence from the Linux runtime's and from the process-native design's per-instance SIDs, and the difference is worth drawing out rather than eliding.

**Mount-graph enforcement is discretionary.** Compartment policy is attached from the host side, so a workload cannot edit its own — pending confirmation, below. But an administrative process can remove it, there is no per-connection authentication (parity with the Linux DNAT model, so no regression), and the host stays default-open to the external network.

**The base image is a supply-chain dependency of a new shape.** The runtime pulls an OS base and stacks it beneath every workload (`wcr[base.image]`, `wcr[compose.chain]`), so a compromised base compromises everything on the host. Integrity rests on the standard OCI layer digest. `docs/threat-model.md` N4 covers image supply chain generally; this is the same class with a single shared blob at the bottom of every workload.

**The log store has one write path by construction** (`wlog[producer.single-path]`), which is the Windows counterpart of the `--log-driver=none` reasoning already in the base model: output reaches the store once, so the store cannot be used as an injection sink by writing the same record twice by different routes. The per-instance log writers are children of the container supervisor rather than of seedlingd; what identity they run as, and whether a workload can interfere with the writer capturing its own output, is unanswered.

## What changed from the process-native model

That design's threat model was deleted with the design. Some of its threats no longer exist and should not be carried across by reflex: there is no seedling-owned control pipe, so supervisor impersonation is gone; there is no shared loopback address space, so address squatting is gone; cross-workload file access is now structural rather than ACL-enforced. What survives in changed form is workload privilege creep, which now turns on the base image rather than on token construction.

## Open questions it cannot be written without

| # | Question | Where it is answered |
|---|----------|---------------------|
| T1 | Can a workload with administrative rights inside its container alter its own compartment's policy? If it can, `wcr[net.dataplane]`'s central claim is false. | An S3 exit criterion. |
| T2 | Can a host ACE naming a container account's SID grant a container process? Sets the floor for `wcr[volume.host-exposure]`. | S6 Q1, container-side experiments — needs a real host. |
| T3 | What identity do the per-instance log writers run as, and can a workload reach its own writer? | Falls out of the log store implementation and L4. |
| T4 | Which base image, and therefore whether workloads are non-administrative by default. | `wcr[identity.workload]` says non-administrative by default, which holds for Nano Server (`ContainerUser`) and not for Server Core (`ContainerAdministrator`). |

T4 is the one that can be settled now, on the identity argument rather than on size, and it should be: several statements in the eventual model are conditional on it.
