# Circuit breaker for runaway evaluations: test cases

## Evaluation time

- [x] A definition with `loop {}`, the operations limit removed and a 1 s limit, fails as a runaway within seconds (verifies spec: LIM)
- [x] An evaluation that returns after spending more than the limit has exceeded it (verifies spec: LIM)
- [x] With the limit at 0, an evaluation spending more than a second is not stopped (verifies spec: LIM)
- [x] Time spent sleeping inside an evaluation does not count toward the limit (verifies spec: LIM)
- [x] An evaluation started inside another shares its time (verifies spec: LIM)
- [ ] A validator that runs away makes `/apps/params/set` fail with `evaluation_runaway`, and `/apps/plan` list it in `rejections` saying so (verifies spec: PARAM, WIRE)
- [ ] An `/apps/update` whose new definition runs away is refused with `evaluation_runaway` and files nothing (verifies spec: WIRE, APPS)

## Breakers

- [x] A tripped breaker refuses its own definition and inputs, and lets other inputs evaluate (verifies spec: LIM)
- [x] A tripped breaker still holds after the database is reopened with memory cleared (verifies spec: LIM)
- [x] A second runaway with other inputs re-records them without clearing and filing the fault again (verifies spec: LIM, FLT)
- [x] An in-time run with other inputs resets the breaker and clears its fault (verifies spec: LIM)
- [x] Clearing the fault resets the breaker (verifies spec: LIM)
- [x] A breaker that cannot be read is never taken for clear, and holds `breaker_state_unreadable` (verifies spec: LIM, FLT)
- [x] A closure the definition no longer declares has its breaker reset (verifies spec: LIM, FLT)
- [x] Raising the limit counts as new inputs (verifies spec: LIM)
- [x] The likely cause names differing inputs, never their values; names the definition when only it differs; and is unknown without an in-time evaluation (verifies spec: FLT)
- [x] Only the definition's and gate bodies' faults are withholding (verifies spec: FLT)

## Tracked evaluations and refusals

- [x] A definition that runs away on a param change files `evaluation_runaway`, not `script_error`; the same params are then not evaluated again; other params apply and clear it (verifies spec: LIM, FLT)
- [x] `/apps/action/invoke` with params its action's breaker is tripped for is refused with `evaluation_runaway`; other params are accepted (verifies spec: ACI, LIM)
- [x] A scheduled run whose action's breaker is tripped is skipped and stamped, with no further fault (verifies spec: ACN, SCHED)
- [x] A scheduled run whose breaker cannot be read is not made and stays due (verifies spec: SCHED)
- [ ] An action pass that runs away trips the action's breaker and fails the operation; a pass at an earlier generation fails without tripping (verifies spec: LIM)
- [ ] `Action.invoke` of an action tripped for those params throws; a replayed recorded `invoke` is not checked (verifies spec: ACN)
- [ ] Opening a shell is refused under `script_error`, under the definition's `evaluation_runaway`, and when the shell's breaker is tripped for its params (verifies spec: SHL)
- [ ] A gate run fails with nothing stopped when its body's breaker is tripped, and an operator gate-run request is refused with `evaluation_runaway` (verifies spec: GRUN, ACTL)
- [ ] A probe reports a tripped handler, and one stopped at the limit, as that handler's error (verifies spec: IMG)
- [ ] Clearing an app's faults while its definition's breaker is tripped evaluates the definition again (verifies spec: LIM)
- [ ] Deregistering an app discards its breakers, so a new app of the same name starts clear (verifies spec: LIM)
