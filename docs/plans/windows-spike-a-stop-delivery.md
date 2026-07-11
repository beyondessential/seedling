# Spike A: Stop Delivery + Job Objects

Budget: an afternoon. Environment: any Windows Server 2019+ box; no field
image needed.

## At stake

- `win[stop.methods]` — the `ctrl_break` / `ctrl_c` methods carry the
  `[spike]` tag this spike exists to remove or amend.
- `win[signal.exit-codes]` — TerminateJobObject exit-code synthesis and the
  negative-code recording convention.
- `win[identity.dynamic-jobs]` — the CreateService+StartService dispatch
  latency measurement feeds the retreat decision recorded in
  `windows-runtime-rationale.md` (stable per-definition service names, or
  parent-SID sharing).
- Q2 (pipe protocol) is *not* in scope here beyond noting anything that
  constrains it.

## Setup

A throwaway supervisor harness: a console process that

1. creates a Job Object and assigns itself nothing;
2. spawns a Node child with `CREATE_NEW_PROCESS_GROUP`, sharing the
   supervisor's console, assigned to the Job;
3. waits on the child and reports exit codes.

The Node child installs `SIGBREAK`/`SIGINT` handlers that log, flush a file
(simulating shutdown work), and exit with a distinctive code.

## Experiments

1. **CTRL_BREAK to the child's group.** `GenerateConsoleCtrlEvent` with the
   child's process-group id. Confirm: the handler runs, shutdown work
   completes, the distinctive exit code is captured by the harness — and
   the supervisor itself does not receive the event.
2. **CTRL_C delivery.** The platform documents CTRL_C as not
   group-targetable the way CTRL_BREAK is. Establish what actually works:
   group-targeted delivery, or group 0 with the supervisor shielding itself
   via its own ctrl handler. Also check whether `CREATE_NEW_PROCESS_GROUP`
   leaves the child with CTRL_C disabled by default and whether the child
   must re-enable it. The outcome decides whether `ctrl_c` survives as a
   distinct stop method in `win[stop.methods]` or collapses into
   `ctrl_break` only.
3. **Exit-code capture across the ladder.** Cooperative exit (distinctive
   code), then `TerminateJobObject` with a chosen completion code: confirm
   every process in the Job reports that code and the harness can record
   the negative-convention value per `win[signal.exit-codes]`.
4. **Ladder timing.** Deliver stop, wait `stop_timeout`, terminate. Confirm
   there is no race where a child that exited cooperatively during the wait
   is then seen as terminated-by-runtime.
5. **Dispatch latency for dynamic Jobs.** In a loop: `CreateService`
   (virtual account) → `StartService` → stop → `DeleteService`. Record p50
   and p95 wall-clock for the register-to-running edge, and note any
   marked-for-delete residue between iterations (fuller ghost testing is
   Spike E's).

## Exit criteria

- CTRL_BREAK cooperative shutdown works exactly as `win[stop.methods]`
  describes, with the supervisor unaffected: remove the `[spike]` tag.
- The CTRL_C question has a definite answer written back into
  `win[stop.methods]` (kept, constrained, or dropped).
- Exit-code synthesis confirmed; no spec change expected.
- Latency numbers recorded in this file with a verdict: per-invocation
  registration is fine, or a retreat from `windows-runtime-rationale.md` is
  triggered.

## If it fails

- Ctrl-event delivery unreliable: `named_event` becomes the primary
  cooperative stop method and the ctrl methods are dropped from
  `win[stop.methods]`; Node workloads get a stop-event shim.
- Dispatch latency unacceptable: first retreat is stable service names per
  `(app, job-definition)`; second is parent-SID sharing — both already
  recorded in the rationale document.
