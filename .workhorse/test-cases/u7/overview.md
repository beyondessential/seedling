# Circuit breaker for runaway evaluations: test cases

## Evaluation time

- [x] A definition with `loop {}`, the operations limit removed and a 1 s limit, fails as a runaway within seconds (verifies spec: LIM)
- [x] An evaluation that returns after spending more than the limit has exceeded it (verifies spec: LIM)
- [x] With the limit at 0, an evaluation spending more than a second is not stopped (verifies spec: LIM)
- [x] Time spent sleeping inside an evaluation does not count toward the limit (verifies spec: LIM)
- [x] An evaluation started inside another shares its time (verifies spec: LIM)
- [x] A validator that runs away makes `/apps/params/set` fail with `evaluation_runaway`, and `/apps/plan` list it in `rejections` saying so (verifies spec: PARAM, WIRE)
- [x] An `/apps/update` whose new definition runs away is refused with `evaluation_runaway` and files nothing (verifies spec: WIRE, APPS)
- [x] `seedling-ctl definitions check` takes `--script-max-evaluation-time`, and reports a definition that runs past it as a `script` error (verifies spec: CLI, LIM)

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
- [x] An action pass that runs away trips the action's breaker and fails the operation; the same run then fails at once (verifies spec: LIM)
- [x] A pass at an earlier generation is stopped at the limit without tripping a breaker (verifies spec: LIM)
- [x] A gate body that runs away trips the gate's breaker (verifies spec: LIM)
- [x] A shell run that runs away trips the shell's breaker, and its fault names the shell (verifies spec: LIM, FLTS)
- [x] `Action.invoke` of an action tripped for those params throws without running it (verifies spec: ACN)
- [x] A replay reaching a recorded `invoke` of an action since tripped runs it, without checking the breaker (verifies spec: ACN)
- [x] Opening a shell is refused under `script_error`, under the definition's `evaluation_runaway`, and when the shell's breaker is tripped for its params (verifies spec: SHL)
- [x] A gate run fails with nothing stopped when its body's breaker is tripped (verifies spec: GRUN)
- [x] An operator gate-run request is refused with `evaluation_runaway` while the body's breaker is tripped for the run (verifies spec: CTL)
- [x] A probe reports a tripped handler, and one stopped at the limit, as that handler's error (verifies spec: IMG)
- [x] A probe whose capture runs past the limit fails the request with `evaluation_runaway`, not `script_error` (verifies spec: WIRE)
- [x] A probed action or gate body that invokes an action whose breaker is tripped reports the refusal as its error (verifies spec: IMG, ACN)
- [x] Clearing an app's faults while its definition's breaker is tripped evaluates the definition again (verifies spec: LIM)
- [x] An in-time evaluation the runtime adopts resets the definition's breaker, whatever it was tripped for (verifies spec: LIM)
- [x] A breaker fault about an action names it by `resource_type` and `resource_name`; the definition's names nothing (verifies spec: FLTS)
- [x] The web UI withholds Install while the definition has run away, and not while only an action has (verifies spec: UI)
- [x] Deregistering an app discards its breakers, so a new app of the same name starts clear (verifies spec: LIM)
