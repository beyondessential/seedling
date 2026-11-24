# Redact sensitive values wherever the runtime reports them

Build the redaction the X6 spec edits describe. Sensitive values are secret parameter values, secret action and shell param values, and derived values. They are withheld from app descriptions, parameter values, generation history, events, the action execution log, breadcrumbs, and free text such as fault descriptions, error messages and log lines.

## Design notes

- **One matcher type.** `runtime::seeds::DerivedValues` became `SensitiveValues` in `runtime/redact.rs`. Secret param values go into it as well, so every check runs one code path.
- **Masking from hashes.** `mask` finds each matching window, merges overlapping and adjacent byte ranges, and replaces each with `<redacted>`. A window whose bytes equal a valid UTF-8 value begins and ends on character boundaries. One that does not can only be a hash collision, and is passed over.
- **No process-wide registry (drift from the first draft).** The draft keyed a global registry by app name. Tests run many daemons in one process, and they share app names such as `demo`, so a global registry would mix their values and let one daemon's deregistration remove another's. The values travel with the work instead:
  - An operation or shell session holds a `redact::Scope` that starts from its app's values. Its secret params, each invoked action's secret params and its derived values are added as they arrive.
  - The scope is installed on the closure's thread beside the operation seed. Breadcrumbs and the action log read it from there.
  - OI handlers compute the app's values per request with `params::current_sensitive`. It reads stored values from the database rather than the running definition, because a change the definition could not take is stored all the same.
- **Free text is masked where it is produced, not at the fault store.** The only free text that can hold a sensitive value is an evaluation error, a validator rejection, an operation's or shell's failure, or a probe error. Each is masked at its source, so a fault, event or log line built from it is already masked, and every value applies exactly as [Secret storage](../../specs/runtime/secrets.md#matching) scopes it.

- **One check over every response and event (added after three review rounds).** Each round found another member that leaked a value. Each OI response and event is now checked as a whole before it leaves (`oi/redaction.rs`, called from the dispatcher and the event pump). Every string other than an identifier is masked against the sensitive values of the app it concerns. The per-field rules stay where the specs give a `null` marker, and the check catches what they leave out. Identifiers are recognised by member name. Definition and template retrieval are exempt, as the operator's own text.
- **Earlier secret values stay sensitive (decided after review).** An app's sensitive values include every value its generation history records for a parameter that is secret now, or records protected. So a changed password is still masked wherever an older record quotes it.
- **Masking is idempotent.** An occurrence overlapping an existing `<redacted>` marker is passed over, so text masked at its source and again by the whole-response check reads as if masked once, however short the secret.

## Matching and masking

- [x] Create `crates/core/src/runtime/redact.rs`, and move `DerivedValues` there as `SensitiveValues`
- [x] Add `SensitiveValues::mask(&str) -> Cow<str>` and `extend`
- [x] Unit-test `mask`: overlapping and adjacent occurrences, a value among multibyte text, a value equal to a held value, no match
- [x] Add `AppDef::sensitive(stored)` and `App::sensitive()`
- [x] Record operation-seed derived values into the running scope, so an action closure's derived values are no longer dropped

## Scopes

- [x] `redact::Scope`, starting from the app's values, with `insert_secret_params`
- [x] Create the scope in `run_operation_loop` (actions and gate runs) and in the shell session runner, seeded with the validated params' secret values. Pass it in `OperationContext`, and install it with the operation seed
- [x] Grow the scope from `action_call.rs::resolve_call_params` with each invoked action's secret params, on first run and on replay
- [x] The probe masks its error against its app's values and a detached scope of its own

## App description

- [x] Replace `appdef_json::redact_secret_env` with `redact_def_strings`, a walk over every string value in each resource's `def`
- [x] Call it from `describe_app`, and delete the ad-hoc `secret_values` collection. Describe now reads secret param names only, never their values
- [x] Tests: a volume write holding a secret param value and one holding a derived value, `command`, `args` and healthcheck `cmd` elements each nulled on their own, and neighbouring strings left as they are

## Parameter values, history and events

- [x] `describe_app` `params` and `unknown_params`: `value` is `null` where it holds a sensitive value
- [x] Generation history: `redacted` also when either plain value holds a current sensitive value
- [x] `StoredEdit::event_entry` takes the app's values after the change, for `ParamsChanged` and `AppUpdated`
- [x] Tests: a non-secret `db-url` holding the secret password is withheld in describe, history and `ParamsChanged`, including the history entry recorded before the password was set

## Action execution log

- [x] `split_secret_params` seals each non-secret value that holds a sensitive value, and masks it in the journal copy
- [x] Tests: the split itself, and a plain sub-action param holding an operation-derived value is sealed and recovered on replay

## Breadcrumbs

- [x] `BreadcrumbKind::message` masks against the running scope, and masks each `SubAction` string param before serialising it
- [x] Tests: `rt.exec` argv, a `SubAction` value that serialising would escape, and a `UnitCreate` source call
- [x] Settle the `rt.exec` breadcrumb issue filed from V6

## Free text

- [x] Mask evaluation errors and validator rejection reasons in `runtime/apps.rs::run_definition`. That covers `script_error`, its fault, update previews, `validation_error` and `/apps/params/set` failures
- [x] Mask operation and gate run failures in `run_operation_loop`, before the log line, the `operation_failed` fault and `OperationFailed`
- [x] Mask shell closure failures in the session runner
- [x] Tests: a validator `throw` quoting a secret masks the rejection reason, and a closure `throw` quoting a derived value is masked in the failure

## Review follow-ups

- [x] Earlier values of secret parameters join the app's sensitive values (`runtime::apps::sensitive_values`, `generations::earlier_secret_values`)
- [x] Masking leaves an existing marker alone
- [x] Check every OI response and event as a whole, except definition and template retrieval; drop the per-field fault masking it subsumes

## Finish

- [x] Cite `SEC#matching`, `SEC#withheld-values`, `SEC#sensitive-value`, `SEC#redaction`, `APPS#redacted-values`, `APPS#parameters`, `HIST#action-calls` and `ACTU#breadcrumbs` at the code that implements each
- [x] Update `docs/threat-model.md` and `docs/bsl-scripting.md`
- [x] `cargo clippy`, `cargo fmt`, `etc/ci/check-spec-style.sh`, `etc/ci/check-spec-citations.sh`, and the core test suite
