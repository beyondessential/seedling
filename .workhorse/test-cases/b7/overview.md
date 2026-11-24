# B7: Boolean, integer and choice parameter kinds

## Parsers

- [x] `std::parse_boolean` accepts `true` and `false` as booleans and refuses `True`, `FALSE`, `1`, `0`, `yes`, a leading space and the empty string (verifies spec: ACN)
- [x] `std::parse_integer` accepts `0`, `42`, `-7` and both 64-bit limits as integers (verifies spec: ACN)
- [x] `std::parse_integer` refuses `+1`, `01`, `-0`, `1.0`, `1e3`, ` 1`, `-`, `0x10` and the empty string, and a value past the 64-bit range (verifies spec: ACN)
- [x] `std::parse_integer` with an inclusive, exclusive or start-less range accepts only values within it, and its refusal states the range (verifies spec: ACN)
- [x] `std::parse_integer` refuses an empty range and an argument that is not a range (verifies spec: ACN)
- [x] `std::parse_choice` accepts an option exactly, refuses one differing in case, and throws without `options` (verifies spec: ACN)
- [x] `std::parse_choice` refuses empty, duplicate, empty-string and multi-line options and a non-string option (verifies spec: ACN)
- [x] The new parsers refuse a control character like every single-line kind (verifies spec: ACN)

## Declaring kinds

- [x] `param.kind` takes a kind name and a kind map, with and without arguments (verifies spec: PARAM, ACN)
- [x] `param.kind` refuses an unknown key, an argument of another kind, an empty range, `choice` without options, a map without `name`, an unknown name and `volume` (verifies spec: ACN, PARAM)
- [x] `default_value` takes a boolean or integer as its canonical form, `""` clears it, and a float throws (verifies spec: PARAM)
- [x] A default the kind rejects, or a native default of the wrong kind, fails the evaluation whichever of `kind` and `default_value` came first (verifies spec: PARAM)
- [x] An action param default its kind rejects throws at declaration; native defaults and an empty default are taken (verifies spec: ACN)

## Values in scripts

- [x] `param.parsed()` gives a boolean, an integer, the chosen option and the email parts (verifies spec: PARAM)
- [x] `param.parsed()` throws the parser's error for a stored value the kind now rejects, and throws when the param is unset (verifies spec: PARAM)
- [x] Validators receive parser results, and `all` holds only declared, set parameters (verifies spec: PARAM)
- [x] No validator runs while any parameter's kind rejects its value (verifies spec: PARAM)

## Stored parameters

- [x] `/apps/params/set` stores a JSON boolean or integer as its canonical text, and the same value as text changes nothing (verifies spec: PRM)
- [x] An out-of-range, non-canonical or unknown-option value fails validation, as does a native value for a kind not its own or for an undeclared parameter (verifies spec: PRM)
- [x] A JSON float, exponent or out-of-range number is refused as `requirements_invalid` by `/apps/params/set`, `/apps/plan` and `/apps/update` (verifies spec: PRM, ACI)
- [x] `/apps/plan` and `/apps/update` take native values as `/apps/params/set` does (verifies spec: PRM, APPS)

## Action and shell params

- [x] A JSON boolean or integer is taken for the matching kind, and a native value for another kind, or an array, is refused before defaults apply (verifies spec: ACN, ACI)
- [x] `1.0`, `1e3` and numbers past the 64-bit range are refused for an integer param (verifies spec: ACI)
- [x] Declared params reach the closure as parser results, including a `Version`, and undeclared ones as supplied (verifies spec: ACN)
- [x] A declared param with no value is absent from the param map (verifies spec: ACN)
- [x] An invoked closure receives a boolean, integers, the email map and a `Version` (verifies spec: ACN)
- [x] `invoke` refuses a native value for a kind that is not its own (verifies spec: ACN)
- [x] A closure can pass on the param map it received, versions included (verifies spec: ACN)
- [x] A scheduled run's closure receives its params' defaults (verifies spec: ACN)
- [x] An operator-invoked action, end to end, delivers a JSON boolean, a JSON integer and a choice to its closure as parser results, and refuses a native value of the wrong kind (verifies spec: ACI)
- [x] A shell's JSON boolean param reaches its closure as a boolean, and a native value of the wrong kind is refused (verifies spec: SHL)

## Persistence and replay

- [x] A persisted operation's params round-trip with `Version` and `SemVer` restored as their types, and a purl's version restored as a `Version` (verifies spec: OPS)
- [x] A recorded `invoke` with a version param replays with the recorded version, not the value the script now supplies (verifies spec: OPS)

## Probes

- [x] A probe passes parser results to the closure (verifies spec: IMG)
- [x] A probe reports a kind rejection as an error in strict mode and a skip in lenient mode, naming the parameter and the parser's error (verifies spec: IMG)

## Wire reporting

- [x] `/apps/show` reports `range` with `null` for an unbounded end of `1..`, `-3..=` and `..5`, and `null` without a range (verifies spec: APPS)
- [x] `/apps/show` reports `options` in declared order, for stored params and action params alike (verifies spec: APPS)
- [x] Native defaults are reported as their canonical text (verifies spec: APPS)
- [x] A template preview's params carry `range` and `options` (verifies spec: TPL)

## Web console

- [x] A boolean param offers a toggle, and an optional one can be cleared by toggling its value off (verifies spec: ACN)
- [x] An integer param offers a number input bounded by its range, holding back an out-of-range value (verifies spec: ACN)
- [x] A choice param offers its options in declared order, with an empty entry when optional (verifies spec: ACN)
- [x] The stored-parameter editor edits a boolean and an integer with their own inputs and submits their text (verifies spec: ACN)
