---
id: LIM
---

# Script engine limits

## Script Engine Limits

<!-- from runtime: Script Engine Limits -->

> r[engine.limits]
> The runtime must constrain the BSL script engine to prevent unbounded resource consumption.

> r[engine.limits.operations]
> The engine must enforce a maximum number of operations per script evaluation.
> When the limit is reached, evaluation must fail with an error.
> The default limit is 100 000 operations.
> This limit may be overridden at startup.

> r[engine.limits.call-depth]
> The engine must enforce a maximum function call nesting depth.
> The default limit is 64.
> This limit may be overridden at startup.

> r[engine.limits.expr-depth]
> The engine must enforce a maximum expression nesting depth.
> The default limit is 64.
> This limit may be overridden at startup.

> r[engine.limits.string-size]
> The engine must enforce a maximum string length in bytes.
> The default limit is 1 048 576 (1 MiB).
> This limit may be overridden at startup.

> r[engine.limits.array-size]
> The engine must enforce a maximum array size in elements.
> The default limit is 10 000.
> This limit may be overridden at startup.

> r[engine.limits.map-size]
> The engine must enforce a maximum object map size in entries.
> The default limit is 10 000.
> This limit may be overridden at startup.
