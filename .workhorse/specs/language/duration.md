---
id: DUR
---

# Durations

Applies to the `duration` [param kind](action.md#param-kinds), its [parser](action.md#kind-parsers), and the `Duration` values that parser returns.

## Borrowed terms

| term | meaning |
| --- | --- |
| friendly duration format | the duration format given by the [friendly format grammar](https://docs.rs/jiff/0.2/jiff/fmt/friendly/index.html#grammar) of the jiff library |

## Terminology

### Duration

A length of time, held as a whole number of nanoseconds from 0 to 2⁶³−1.

## Duration syntax

`std::parse_duration(text)` MUST return a `Duration` for a value it accepts.
It MUST accept a value in the friendly duration format, and MUST reject every other value.
It MUST also reject a value that carries a sign or an `ago` direction, that uses a unit of years or months, or whose length exceeds the largest duration.

A day MUST count as 24 hours, and a week as 7 days.

> [!NOTE]
> The format takes compact and spelled-out forms alike, so `1h30m`, `1h 30m`, `1 hour, 30 minutes` and `01:30:00` are the same duration.
> A fraction is allowed on the last unit only, from hours down, as in `1.5h`.
> A month or a year has no fixed length, so a value meaning about a month is written in days, as `30d`.

## Text of a duration

A `Duration` MUST convert to a string in this form: each unit from days down to nanoseconds whose part of the duration is not zero, largest first, as the count followed by the unit's designator, separated by single spaces.
The designators are `d`, `h`, `m`, `s`, `ms`, `us` and `ns`.
A zero duration MUST convert to `0s`.

> [!NOTE]
> `std::parse_duration("90m")` converts to `1h 30m`, and `std::parse_duration("2w")` to `14d`.
> The string form is always one `std::parse_duration` accepts, giving back an equal duration.

## Reading a duration

| method | returns |
| --- | --- |
| `as_seconds()` | the duration as a whole number of seconds, as an integer |
| `as_millis()` | the duration as a whole number of milliseconds, as an integer |
| `as_seconds_f()` | the duration in seconds, as a floating-point number |
| `as_millis_f()` | the duration in milliseconds, as a floating-point number |

`as_seconds()` MUST throw when the duration is not a whole number of seconds, and `as_millis()` when it is not a whole number of milliseconds.

> [!NOTE]
> The integer methods suit an application setting that takes a count of one unit, such as a timeout in milliseconds given in an environment variable.
> The floating-point methods never throw, and may round a duration too long for a floating-point number to hold exactly.

## Comparing durations

Two `Duration` values MUST compare by length through `<`, `<=`, `>`, `>=`, `==` and `!=`.
Comparing a `Duration` with a value of any other type, including a string, MUST raise a script error.

## Arithmetic on durations

| expression | result |
| --- | --- |
| `a + b` | the sum of the durations `a` and `b` |
| `a - b` | the difference of the durations `a` and `b` |
| `a * n`, `n * a` | the duration `a` taken `n` times, for an integer `n` |
| `a / n` | the duration `a` divided by an integer `n`, rounded toward zero to a whole nanosecond |

An expression MUST raise a script error where its result would be negative or exceed the largest duration, or where `n` is negative, or where it divides by zero.
