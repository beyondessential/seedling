# H6: effective-value change handlers

## Decisions

- `on_change` fires when a change moves the parameter's effective value (what `value()` returns), and only then. Set, unset and definition update are the same kind of event; pinning a parameter to its current effective value fires nothing.
- A change that would fire more than one handler is refused with a new `multiple_handlers` wire error. Reviewing the one-parameter-per-change restriction as a whole is #M6.
- An operation initiated by a generation bump holds the app's desired state from the commit, including while queued.

## Notes

- Desired-state gap: `crates/core/src/system/reconcile.rs` holds an `Installed` app's desired state only while `active_progress` is set or `start_pending` is true. `active_progress` is first written from the operation's barrier log once the closure runs, so a `param_change` handler that is queued, or admitted but not yet flushed, leaves the new generation's whole graph as the desired state. The hold has to be in place before the new definition becomes visible to the reconciler. It could be a flag like `start_pending`, set under the same step that swaps `app.def`.
- Deciding which handlers fire needs effective values on both sides before persisting: the previous generation's definition at previous values, and the proposed definition at proposed values. Validation already evaluates the proposed side.
- Docs to update: `docs/bsl-scripting.md` (the `on_change` comment near the version example), `docs/runtime-overview.md` (initiating-events table).
