---
id: AUD
---

# Audit log

Applies to the durable record of events the [runtime](../overview.md#runtime) keeps on a [host](../overview.md#host) for later inspection.

## Borrowed terms

| term | meaning |
| --- | --- |
| JSON Lines | a text format holding one JSON value per line, each line ended by a newline ([JSON Lines](https://jsonlines.org/), over JSON as defined in [RFC 8259](https://www.rfc-editor.org/rfc/rfc8259)) |

## Storage

The runtime MUST keep an audit log that it only ever appends to.
The runtime MUST store the audit log apart from its operational records, and [garbage collection](garbage-collection.md) MUST NOT delete audit entries.

The runtime MUST write the audit log to a file at a configurable path.
The default path is `/var/log/seedling/audit.log`, and the daemon option `--audit-log` sets another.
The runtime MUST create the file's parent directory if it does not exist.
The runtime MUST create the file readable and writable by its owner only.

## Entries

The runtime MUST write to the audit log every event it publishes on the [event feed](../interface/events.md#event-types).
The audit log MUST be in JSON Lines, each entry being one event in the event feed's JSON representation, with its `type` and `timestamp` members.

## Generations

An entry recording a change to an app's definition or parameter values, including a registration, a definition update, a parameter set and a parameter unset, MUST include the [generation](../overview.md#generation) the change created.
An entry recording any such change other than a registration MUST also include the generation the change superseded.
An entry recording a [lifecycle operation](../overview.md#lifecycle-operation) that a generation change triggered MUST include the operation's source and target generations.

## Rotation

The runtime MUST support rotation of the audit log by an external tool that renames or removes the file.
After the file at the configured path has been renamed or removed, the runtime MUST write the next entry to a file at the configured path, creating it if necessary.

## Write failures

A failure to open or write the audit log MUST NOT block or stop the runtime.
The runtime MUST report each failure to write an entry in its own log.

If the runtime cannot open the audit log, it MUST file a fault of kind `audit_log_unavailable` against the app name `seedling`, and MUST try to open it again before writing each later entry.
The fault MUST state how many events went unwritten while it was active.
The runtime MUST clear that fault once it has opened the audit log.

If events are lost to the audit log because the runtime fell behind in writing them, the runtime MUST hold a [fault](../overview.md#fault) of kind `audit_lag` against the app name `seedling`, giving the number of events lost.
At most one such fault MUST be active at a time.
The fault MUST stay active until an operator [clears](../interface/faults.md#clearing-faults) it.
