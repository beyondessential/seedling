---
id: AUD
---

# Audit log

## Audit Log

<!-- from runtime: Audit Log -->

> r[audit.log]
> The runtime must maintain a durable, append-only audit log capturing significant system events. The audit log is stored separately from operational data so that operational tables can be garbage-collected without losing the historical record.

> r[audit.log.path]
> The audit log must be written to a configurable file path, defaulting to `/var/log/seedling/audit.log`. The runtime must create the parent directory if it does not exist.

> r[audit.log.format]
> Each audit log entry must be a single line of JSON (JSON Lines format). Each entry must include a timestamp and an event type.

> r[audit.log.events]
> The audit log must record at minimum:
>
> - App registration and deregistration.
> - Lifecycle operation start, completion, and failure.
> - Faults filed and cleared.
> - Resource state transitions.
> - Site volume creation, deletion, snapshotting, and promotion.
> - External volume mapping creation, deletion, and retargeting.
> - Template creation, removal, and instantiation.

> r[audit.log.generations]
> Audit entries that record changes to an app's defined state — including registration, script update, parameter set, and parameter unset — must include the [generation](rule:r/generation.definition) of the change.
> For changes that supersede a prior generation (any change other than initial registration), the entry must also include the previous generation.
> Lifecycle operation audit entries triggered by a generation change must include the source and target generations from the operation record.

> r[audit.log.rotation]
> The audit log file must be compatible with external log rotation tools. The runtime must detect when the log file has been rotated (renamed or removed) and reopen the configured path.

> r[audit.log.resilience]
> Failure to write to the audit log must not block or crash the runtime. Audit write failures must be reported via tracing.
