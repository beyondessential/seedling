# Redact sensitive values wherever the runtime reports them

Build the redaction the X6 spec edits describe. Sensitive values are secret parameter values, secret action and shell param values, and derived values. They are withheld from app descriptions, parameter values, generation history, events, the action execution log, breadcrumbs, and free text such as fault descriptions, error messages and log lines.

## Design notes

- **One matcher type.** `runtime::seeds::DerivedValues` already holds keyed hashes by length and answers `found_in`. It becomes `SensitiveValues` in a new `runtime/redact.rs`, and secret param values go into it as forms too, so every check runs one code path. Rename every use. Leave no alias.
- **Masking from hashes.** `mask` finds each matching window, merges overlapping and adjacent byte ranges, and replaces each with `<redacted>`. A window whose bytes equal a valid UTF-8 form begins and ends on character boundaries, so splicing by byte range is sound.
- **A process-wide registry, keyed by app.** It holds the app's set and one set per live operation or shell session. Breadcrumbs (`system/actuator/pod.rs` emits one with only an app name), fault filing (`FaultKey` carries the app) and event emission are each reached from many sites that hold no `App`. A registry looked up by app name lets each of them mask at one chokepoint, instead of threading a set through every caller.
- **Free-text chokepoints mask against the union of the app's sets.** This includes every live operation's and session's set, not only the one that recorded the text. That over-redacts, which fails safe ([Secret storage](../../specs/runtime/secrets.md#matching) accepts over-redaction already), and spares each chokepoint from working out which scope the text came from.
- **Structured fields check at report time.** History, describe and event payloads match against the app's set as it stands when the response or event is built. Nothing about redaction is stored with a value except the existing ciphertext columns.

## Matching and masking

- [ ] Create `crates/core/src/runtime/redact.rs`, and move `DerivedValues` there as `SensitiveValues`, with its hasher, `insert`, `len`, `is_empty` and `found_in`. Update `AppDef::derived`, the seed frames in `runtime/seeds.rs`, `defs/stdlib/random.rs`, and the tests in `runtime/seeds/tests.rs` and `tests/stdlib.rs`
- [ ] Add `SensitiveValues::mask(&str) -> Cow<str>`, borrowing when nothing matches, and `extend(&SensitiveValues)` for combining sets
- [ ] Unit-test `mask`: overlapping and adjacent occurrences, a form inside a multibyte string, a value equal to a form, no match
- [ ] Add `App::sensitive() -> SensitiveValues`. It holds the `App.stored` values of the params `def.params` marks secret, plus `def.derived`
- [ ] Record operation-seed derived values. `OperationSeedFrame` installs a `DERIVED` set as `AppSeedFrame` does, and `random.rs::record` records in both contexts, so an action closure's derived values are no longer dropped

## Registry

- [ ] In `redact.rs`, add the registry `AppName -> { app: SensitiveValues, scopes: HashMap<ScopeId, SensitiveValues> }` with `withholds(app, text) -> bool` and `mask(app, text) -> Cow<str>`, both over the union
- [ ] Refresh the app entry wherever an evaluation is applied to `App.def`, and after `params::store_edits` commits. Remove it on [deregistration](../../specs/runtime/app-teardown.md#deregistration)
- [ ] Register an operation scope when `oi/handler/actions/lifecycle.rs` and `runtime/gates/run.rs` start a pass. Seed it with the secret params of the operation's validated params (supplied or defaulted)
- [ ] Grow the operation scope from `action_call.rs::resolve_call_params` with each invoked action's secret params, and from `seeds::record_derived` with each operation-seed derived value as it is recorded
- [ ] Register a shell session scope in `oi/shells/session.rs` when the session opens, from its validated params and its session-seed derived values. Drop it when the session ends
- [ ] Drop an operation scope only after its final event, fault and log line are emitted, on completion, failure and cancellation alike

## App description

- [ ] Replace `appdef_json::redact_secret_env` with a walk over every string value in each resource's `def`: object member values and array elements at any depth, not keys. Null each one the app's set withholds
- [ ] Call it from `describe_app` with `App::sensitive()`, and delete the ad-hoc `secret_values` collection in `oi/handler/apps.rs` around the `APPS#redacted-values` citation
- [ ] Tests in `oi/handler/apps/tests.rs`: a volume write holding a secret param value, a `command`/`args` element and a healthcheck `cmd` element each nulled on their own, a derived value in a write, and untouched neighbouring strings

## Parameter values, history and events

- [ ] `describe_app` `params`: report `value` as `null` where the app's set withholds it. Do the same for `unknown_params`
- [ ] Generation history (`oi/handler/apps.rs`, where `redacted` is set from `previous_value_redacted`): also set `redacted` when either plain value is withheld by the app's current set
- [ ] `StoredEdit::event_entry` takes the app's set after the change. `ParamsChanged` (`oi/handler/params.rs`) and `AppUpdated` (`oi/handler/apps.rs`) pass the set of the evaluation the change produced
- [ ] Tests: a non-secret `db_url` containing the secret password is withheld in describe, history and `ParamsChanged`. A history entry recorded before the password was set is withheld once it is set

## Action execution log

- [ ] `action_call.rs::split_secret_params` also moves into the sealed `secret` map each non-secret value whose JSON text the app or operation set withholds. The journal copy of such a value is masked rather than replaced whole
- [ ] Tests in `action_call.rs`: a plain param holding an operation-derived value is sealed, and replay recovers it

## Breadcrumbs

- [ ] Split `Breadcrumb::emit` so the message is built by a pure function that masks through the registry, and so it can be tested without journald. Mask each `SubAction` string param before JSON serialisation, since escaping could otherwise split a match
- [ ] Tests: `rt.exec` argv holding a secret param and a derived password, an anonymous resource summary whose command head holds one, and a `UnitCreate` source call
- [ ] Settle the `rt.exec` breadcrumb issue filed from V6 with this step's commit

## Free text

- [ ] Mask fault descriptions in `runtime/faults.rs::file_keyed`, the one insert path behind `file_once` and `sync_faults`
- [ ] Mask the free-text members of app-scoped events at emission: `OperationFailed.error`, fault event descriptions, and any other free-text member found by auditing `crates/protocol/src/events.rs`. Install the hook from core on the protocol crate's sender, since that crate cannot see the registry
- [ ] Mask BSL-sourced text in OI errors: `validation_error` and the update preview's `rejections[].reason`, the describe `script_error`, `/apps/params/set` validation failures, and the operation failure text passed to `op_ctx.failed`
- [ ] Mask the tracing lines that log operation failures and evaluation errors: `lifecycle.rs` around `op_ctx.failed`, `gates/run.rs`, and the evaluation error paths in `runtime/apps.rs`
- [ ] Tests: a validator `throw` quoting a secret value masks the rejection reason. A closure `throw` quoting a derived value masks `OperationFailed.error` and the fault description

## Finish

- [ ] Cite `SEC#matching`, `SEC#withheld-values`, `SEC#redaction`, `APPS#redacted-values`, `APPS#parameters`, `APPS#generation-history`, `HIST#action-calls` and `ACTU#breadcrumbs` at the code that implements each
- [ ] Update `docs/threat-model.md` (redaction bullets) and `docs/bsl-scripting.md` (derived values withheld) for the wider coverage
- [ ] `cargo clippy`, `cargo fmt`, `etc/ci/check-spec-style.sh`, `etc/ci/check-spec-citations.sh`
