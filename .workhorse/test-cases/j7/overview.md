# Duration parameter kind

## Parsing

- [x] `1h30m`, `1h 30m`, `1 hour, 30 minutes`, `90 mins`, `01:30:00`, `1.5h` and `5400s` are accepted as the same duration (verifies spec: DUR#duration-syntax)
- [x] `1d` is 24 hours and `1w` is 7 days (verifies spec: DUR#duration-syntax)
- [x] `-5s`, `+5s`, `5s ago`, `1mo`, `1y` and a zero calendar unit such as `0mo 5d` are refused (verifies spec: DUR#duration-syntax)
- [x] `PT1H30M`, a bare number, an unknown unit, a repeated unit and the empty string are refused (verifies spec: DUR#duration-syntax)
- [x] A value holding a line break or other control character is refused (verifies spec: DUR#duration-syntax)
- [x] A length past 2⁶³−1 nanoseconds is refused (verifies spec: DUR#duration)

## Duration values

- [x] The text form runs from days down with `us`, `90m` gives `1h 30m`, `2w` gives `14d` and zero gives `0s`, and every text form parses back to an equal duration (verifies spec: DUR#text-of-a-duration)
- [x] `as_seconds()` and `as_millis()` throw on a fraction of their unit; `as_seconds_f()` and `as_millis_f()` never throw (verifies spec: DUR#reading-a-duration)
- [x] Durations compare by length; comparing one with a string or a number throws (verifies spec: DUR#comparing-durations)
- [x] `+`, `-`, `*` in both orders and `/` give the expected durations; a negative result, an overflow, a negative factor and a division by zero throw (verifies spec: DUR#arithmetic-on-durations)

## The kind

- [x] `kind(#{ name: "duration", min, max, precision })` takes each argument, as a string or a `Duration`; a number, an unknown designator, an unknown argument, `min` past `max`, and bounds holding no whole unit of the precision throw (verifies spec: DUR#bounds)
- [x] `std::parse_duration(v, #{ ... })` refuses a value outside its bounds, stating the bound broken (verifies spec: DUR#bounds)
- [x] `param.parsed()` and an action's param map give a `Duration` (verifies spec: ACN#param-kinds)
- [x] `default_value` takes a `Duration` and stores its text form; a `Duration` default on another kind fails the evaluation (verifies spec: PARAM)
- [x] `/apps/params/set` and an action invocation refuse a value outside the bounds or off the precision (verifies spec: DUR#bounds)
- [x] `/apps/show` and `/templates` report a duration param's `range` as text-form strings and its `precision` (verifies spec: APPS)
- [x] A sub-action given a `Duration` param records its text form and replays it as a `Duration` (verifies spec: ACN#action-invocation)

## Duration options

- [x] `stop_timeout`, a healthcheck's `interval`, `timeout` and `start_period`, `job.deadline` and a gate check's `interval` take a string or a `Duration`, refuse a number and refuse a fraction of a second (verifies spec: DUR#duration-arguments)
- [x] Balance `try_duration` and `interval`, and the rate-limit `window` from 1ms to 1h, take duration strings (verifies spec: DUR#duration-arguments)
- [x] A barrier deadline and `rt.stop`'s deadline take a whole-seconds duration; a zero deadline and a number are refused (verifies spec: RT)
- [x] The bundled app definitions evaluate with their options written as duration strings

## Console

- [x] A duration param's helper line shows its bounds and precision before its description, such as "1s to 1d, whole seconds" (verifies spec: APPS)
