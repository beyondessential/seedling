# Spike S6: host filesystem behaviour under containers

Two questions about how the host filesystem behaves at the container boundary. They are unrelated in subject and identical in method — both are "what can a host process do to a file the container side is using, and vice versa" — so they share a harness and a run.

Environment: a Windows Server 2019+ host with the Containers feature, containerd and `ctr` installed, and a base image matching the host build. Run elevated — `icacls` and `ctr` both need it. Harness: `spike-host-fs`.

## What CI already answers

Containers cannot run on a hosted Windows runner — container operations are supported only on Linux runners, and a hosted Windows runner can build a Windows image but not pull or run one. So every experiment below that needs a container waits for a real host.

The rest does not need one. Q2 is pure filesystem, and Q1's first two experiments are `icacls` against a temporary directory, so both run as tests in the Windows CI job on every push. Three of them assert, because the design rests on their answers and a future Windows release changing one should fail loudly rather than be discovered during implementation: that an actively written segment resists an external rename and delete, that a reader following a segment cannot block an append, and that a container account name does not resolve on the host. The fourth — whether a literal container SID is accepted in a host ACL — only records, because its answer is meaningful only alongside the container-side access test that CI cannot run.

That leaves the genuinely host-dependent part small: whether a container process is granted access by an ACE naming its SID, and whether two containers are indistinguishable to it.

## Q1: can a host ACL name a container account?

`wcr[volume.boundary]` states that a volume's host permissions cannot distinguish one instance from another, because a container's account is not a host principal. That follows from Microsoft's guidance, which is to grant a well-known group such as `Authenticated Users` on a bind-mounted directory since the container identities "exist only within the container context, not on the host machine".

What that guidance does not settle is whether the *SID* is usable even though the *name* does not resolve. `ContainerUser` and `ContainerAdministrator` have well-known SIDs in the `S-1-5-93-2-x` range. Windows access checks compare SIDs, not names, so an ACE naming that SID literally may well be honoured for a container process even on a host where the account cannot be looked up.

This does not buy per-instance isolation either way — every container from the same base presents the same SID — but it changes the floor. If it works, a volume can be granted to "container processes" rather than to every authenticated principal on the host, which is a materially tighter default and narrows what `wcr[volume.host-exposure]` has to concede.

### Experiments

1. **Resolve.** Attempt to look up the container SIDs by name on the host, and to convert the literal SID strings to binary form. Expect name lookup to fail and conversion to succeed — that gap is the premise of the whole question.
2. **Apply.** Create a directory, break inheritance, and set a DACL granting only the container SID plus Administrators. Confirm the ACE persists and reads back as an unresolved SID rather than being rejected or silently dropped.
3. **Access.** Bind-mount that directory into a process-isolated container and have the workload read and write a file in it. Success means the host honoured an ACE for a principal it cannot name; failure means `Authenticated Users` is genuinely the floor.
4. **Confine.** Confirm the negative case: a directory granting only Administrators is *not* accessible from the container. Without this, experiment 3 proves nothing — it could be succeeding for an unrelated reason.
5. **Discriminate.** Run two containers against the same directory. Both are expected to have identical access; this records the absence of per-instance granularity as an observation rather than an assumption.

### Exit criteria

- A definite answer on whether a host ACE naming a container SID is honoured for a container process.
- If yes: `wcr[volume.host-exposure]` narrows from "any authenticated host principal" to "any container process", and the volume ACL default changes accordingly.
- If no: the rule stands as written, and `Authenticated Users` is recorded as forced rather than chosen.
- Either way, experiment 5's result is the evidence behind `wcr[volume.boundary]`'s claim that host permissions cannot separate instances.

## Q2: what can a second process do to a segment the log writer holds open?

`wlog[retain.rollover]` puts segment rollover in the log writer rather than in seedlingd, on the grounds that another process cannot reliably rename or delete a file while its writer holds it open. The whole division of labour in `docs/plans/windows-log-store.md` rests on that, so it should be measured rather than assumed.

The nuance is `FILE_SHARE_DELETE`. A writer that opens its segment without it blocks any rename or delete outright. A writer that opens *with* it permits them, but the semantics of the name afterwards differ from POSIX — the classic rename-the-open-file trick behaves differently, and whether the old name becomes immediately reusable depends on how the delete was requested.

### Experiments

1. **No sharing.** Open a file for append without `FILE_SHARE_DELETE`; from a second process attempt rename and delete. Expect both to fail with a sharing violation.
2. **With sharing.** Reopen permitting delete sharing; retry rename and delete. Record which succeed.
3. **After rename.** Where rename succeeded, confirm whether the writer's subsequent appends follow the file to its new name, and whether the original name is immediately free for a new file. This is the behaviour that decides whether a logrotate-style rename-then-reopen is even expressible here.
4. **After delete.** Where delete succeeded, confirm whether the writer's handle stays valid and where its appends go, and whether the name is reusable before the handle closes.
5. **Reader interference.** Confirm a reader can open the active segment for reading, and read to the current end, while the writer is appending — `wlog[read.follow]` requires that following never blocks a producer.

### Exit criteria

- A clear statement of which operations a second process can perform on an actively written segment, under each sharing mode.
- Confirmation that rollover must belong to the writer (or, if the platform turns out to permit a clean external rename, the freedom to revisit that split).
- Confirmation that a follow reader cannot block an append.

## If either answer surprises us

Q1 answering yes is the good case and costs nothing but a tighter default. Q1 answering no is already the assumed position, so nothing moves.

Q2 is the one with structural consequences. If an external rename turns out to be clean, seedlingd could own rollover as well as retention and the writer shrinks further — worth taking, since it is one less thing in the per-instance process. If, less likely, a follow reader can block an append, the read path needs a copy step and `wlog[read.follow]` needs revisiting before anything is built on it.
