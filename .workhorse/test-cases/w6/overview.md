# Set up the std module for BSL helpers

## The std module

- [x] `std::nope()` in the static context raises a function-not-found error naming `std::nope`, where an unregistered module raises module-not-found (verifies spec: STD)
- [x] `std::nope()` in an action closure fails the action with function-not-found (verifies spec: STD)
- [x] `std::nope()` in the validator context raises function-not-found (verifies spec: STD)
- [x] A script-defined function, closure variable or map named like a helper is not reached by `std::nope()` (verifies spec: STD)
- [x] `std::nope()` in a gate body fails the gate run with function-not-found (verifies spec: STD)

## Replicability

- [x] `timestamp()` is not in a script's scope (verifies spec: BSL)
- [x] `sleep(60)` and `sleep(60.0)` throw without pausing evaluation (verifies spec: LIM)
- [ ] `import` is refused, including an `import` whose alias is never used (verifies spec: BSL)
