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

## Rotation

The runtime MUST support rotation of the audit log by an external tool that renames or removes the file.
After the file at the configured path has been renamed or removed, the runtime MUST write the next entry to a file at the configured path, creating it if necessary.

## Write failures

The runtime MUST NOT block or stop because it fails to open or write the audit log.
The runtime MUST report each failure to write an entry in its own log.

If the runtime cannot open the audit log, or cannot write an entry to it, it MUST file an [`audit_log_unavailable`](faults.md#reporting-faults) fault.
While the fault is active, the runtime MUST try again, opening the file afresh, with each later entry.

The runtime MAY drop entries it has fallen behind in writing, rather than block.
Whenever it drops entries, the runtime MUST hold an [`audit_lag`](faults.md#reporting-faults) fault.
