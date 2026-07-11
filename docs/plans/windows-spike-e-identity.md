# Spike E: Identity Mechanics

Environment: a box carrying the hardening GPO baseline
from the field (or the worst-case image from Spike C); the GPO interaction
is the point.

## At stake

- `win[identity.virtual-account]` — virtual-account logon under a hardened
  GPO baseline, and deterministic SID computation before first start.
- `win[identity.scm-entry]` — seedlingd itself under its own virtual
  account (settled decision; this spike validates it holds under GPO).
- `win[identity.non-admin]` — the stripped-token spawn.
- `win[identity.lifecycle]` / `win[identity.gc]` —
  DeleteService-with-open-handles ghost behaviour and the GC sweep for it.
- `win[identity.file-permissions]` — breaking NTFS ACE inheritance on
  volume creation.

## Experiments

1. **Virtual-account logon under GPO.** Register a demand-start service
   with a `NT SERVICE\` virtual account on the hardened image and start
   it. Specifically check whether the baseline's "Log on as a service"
   policy strips the implicit grant virtual accounts normally receive —
   this is the known failure mode. Repeat for a seedlingd-shaped auto-start
   service.
2. **Deterministic SID.** Compute the service SID from the service name
   before registration; create ACLs and WFP filters referencing it; then
   register and start the service and confirm the grants apply. This
   validates the create-grants-before-first-start ordering of
   `win[identity.dynamic-jobs]`.
3. **Stripped-token spawn.** From the service process, derive a restricted
   token (no extra privileges) and `CreateProcessAsUser` a child inside a
   Job. Enumerate both tokens and confirm the child's is strictly
   narrower; confirm a representative workload (Node) functions under it.
4. **DeleteService ghosts.** Delete a service while handles remain open;
   observe the marked-for-delete state; confirm what re-registration under
   the same name does while ghosted; close the handles and confirm
   reaping. Then implement the GC probe: enumerate `seedling-`-prefixed
   registrations and reliably distinguish live, ghosted, and orphaned
   (`win[identity.gc]`).
5. **ACE inheritance break.** Create a volume directory, break inheritance
   at creation, apply the four-principal ACL set (instance SID, daemon
   SID, SYSTEM, Administrators). Confirm: a fifth principal (another
   instance's SID) is denied; a file created deep inside by the workload
   does not pick up inherited access from above the volume root; and
   Administrators can still take ownership (expected, per the threat
   model's WN1).

## Exit criteria

- Virtual-account logon works under the fleet GPO baseline, or the exact
  GPO exception required is documented as a deployment prerequisite and a
  `seedling doctor` probe.
- Grants-before-start ordering validated end-to-end.
- Stripped-token spawn confirmed with a working workload.
- Ghost detection good enough to implement `win[identity.gc]` without
  heuristics.
- Inheritance break confirmed leak-free.

## If it fails

- Virtual accounts blocked by GPO with no acceptable exception: plain
  local accounts stay rejected (audit noise, password-age interactions —
  see the rationale document); the remaining option is fewer identities
  (per-app rather than per-instance), which weakens the ACL/WFP model and
  must go back through a spec session.
- Ghost reaping unreliable: registration names gain a generation suffix so
  a ghost never blocks a fresh registration; the sweep then only has to
  reap, not unblock.
