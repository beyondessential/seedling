---
id: LIM
---

# Script engine limits

Applies to the bounds the [runtime](../overview.md#runtime) places on each evaluation of a BSL script.

## Limits

The runtime MUST bound the resources a single evaluation of a BSL script can consume, with each of the following limits.

| limit | unit | default | daemon option |
| --- | --- | --- | --- |
| operations per evaluation | operations | 100 000 | `--script-max-operations` |
| function call nesting depth | calls | 64 | `--script-max-call-depth` |
| expression nesting depth | levels | 64 | `--script-max-expr-depth` |
| string length | bytes | 1 048 576 (1 MiB) | `--script-max-string-size` |
| array length | elements | 10 000 | `--script-max-array-size` |
| object map size | entries | 10 000 | `--script-max-map-size` |

An evaluation that exceeds any limit MUST fail with an error.

The expression nesting limit MUST apply both at the top level of a script and within function bodies.

An operator MAY override each limit with its daemon option when the daemon starts.
Setting the operations, expression nesting, string, array or object map limit to 0 MUST remove that limit.
