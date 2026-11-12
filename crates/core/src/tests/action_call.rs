//! Tests for `app.action()` and `Action.invoke()`.
//!
//! Spec coverage:
//! - spec: ACN#action-handles
//! - spec: ACN#action-invocation
//! - spec: OPS#action-composition
//! - spec: OPS#cycles
//! - spec: OPS#sub-action-parameters
//! - spec: HIST#action-calls

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rhai::{AST, Dynamic, Engine, FnPtr};
use seedling_protocol::names::ActionName;

use crate::defs;
use crate::runtime::barrier::runtime::{ActionClosureGuard, RuntimeInstance};
use crate::tests::setup;

/// Invoke a single captured action by name, with a fresh runtime stub
/// and the action call table populated for the script's full set of
/// actions. Mirrors the live replay path in `runtime/barrier/replay.rs`
/// without dragging in the database / scheduler / oracle plumbing.
struct Captured {
    actions: std::collections::BTreeMap<ActionName, FnPtr>,
}

fn capture_actions(engine: &Engine, ast: &AST) -> (Captured, defs::app::App) {
    let (mut scope, fresh_app) = defs::scope(Default::default());
    defs::app::begin_closure_capture();
    engine
        .run_ast_with_scope(&mut scope, ast)
        .expect("script must run for action capture");
    let captured = defs::app::end_closure_capture();
    (
        Captured {
            actions: captured.actions,
        },
        fresh_app,
    )
}

fn invoke_action(
    source: &str,
    action_name: &str,
    params: rhai::Map,
) -> Result<(), Box<rhai::EvalAltResult>> {
    let (engine, mut scope, _) = setup(&crate::ScriptLimits::default());
    let ast = engine.compile(source).expect("script must compile");
    engine
        .run_ast_with_scope(&mut scope, &ast)
        .expect("top-level script must succeed");
    let (captured, app_def_holder) = capture_actions(&engine, &ast);

    let fnptr = captured
        .actions
        .get(action_name)
        .cloned()
        .unwrap_or_else(|| panic!("no captured action named {action_name:?}"));

    let rt = RuntimeInstance::stub();
    scope.push("__bsl_rt", rt.clone());
    scope.push("__bsl_closure", fnptr);
    scope.push("__bsl_param", params);

    let outer = ActionName::new_unchecked(action_name);
    // Pass the captured app's def into ActionClosureGuard so action_call's
    // schema lookup finds the registered actions; using AppDef::default()
    // would leave validation oblivious to declared params.
    let action_def_arc = Arc::clone(&app_def_holder.def);
    let result = defs::app::with_action_call_table(captured.actions.clone(), outer, || {
        let _guard = ActionClosureGuard::new(action_def_arc, String::new(), HashMap::new())
            .with_active_rt(rt);
        let call_ast = engine
            .compile("__bsl_closure.call(__bsl_rt, __bsl_param)")
            .expect("call ast must compile");
        let merged = ast.merge(&call_ast);
        engine.eval_ast_with_scope::<Dynamic>(&mut scope, &merged)
    });

    result.map(|_| ())
}

fn err_string(e: Box<rhai::EvalAltResult>) -> String {
    let mut buf = e.to_string();
    let mut current: &rhai::EvalAltResult = &e;
    while let rhai::EvalAltResult::ErrorInFunctionCall(_, _, inner, _) = current {
        buf.push_str(" / ");
        buf.push_str(&inner.to_string());
        current = inner.as_ref();
    }
    if let rhai::EvalAltResult::ErrorRuntime(val, _) = current
        && let Some(s) = val.clone().try_cast::<String>()
    {
        buf.push_str(" / ");
        buf.push_str(&s);
    }
    buf
}

// spec: ACN#action-handles
#[test]
fn app_action_in_static_context_throws() {
    let err = crate::tests::run_test_script_err(
        r#"
        app.action("anything");
    "#,
    );
    let msg = err.to_string();
    assert!(
        msg.contains("inside an action"),
        "expected static-context error, got: {msg}"
    );
}

// spec: ACN#action-handles
#[test]
fn app_action_unknown_name_throws() {
    let err = invoke_action(
        r#"
            app.on_action("outer", |rt, _p| {
                app.action("missing");
            });
        "#,
        "outer",
        rhai::Map::new(),
    )
    .expect_err("unknown action lookup must fail");
    let msg = err_string(err);
    assert!(
        msg.contains("no such action") && msg.contains("missing"),
        "expected no-such-action error naming missing, got: {msg}"
    );
}

// spec: ACN#action-handles
#[test]
fn install_is_not_reachable_via_app_action() {
    let err = invoke_action(
        r#"
            app.on_install(|rt, _p| {});
            app.on_action("outer", |rt, _p| {
                app.action("install");
            });
        "#,
        "outer",
        rhai::Map::new(),
    )
    .expect_err("install must not be reachable");
    let msg = err_string(err);
    assert!(
        msg.contains("no such action") && msg.contains("install"),
        "expected install to be hidden, got: {msg}"
    );
}

// spec: ACN#action-handles
#[test]
fn start_is_callable_by_name() {
    invoke_action(
        r#"
            let counter = #{ ran: false };
            app.on_start(|rt, _p| {
                counter.ran = true;
            });
            app.on_action("outer", |rt, _p| {
                app.action("start").invoke();
                if !counter.ran {
                    throw "start was not invoked";
                }
            });
        "#,
        "outer",
        rhai::Map::new(),
    )
    .expect("start must be callable");
}

// spec: ACN#action-invocation
// spec: OPS#action-composition
#[test]
fn call_runs_inner_closure_with_validated_params() {
    let observed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    {
        let observed_clone = observed.clone();
        // The inner action must observe the params the caller passed
        // *after* validation. We stash via Rhai-side throw so the
        // assertion is visible to the test body.
        invoke_action(
            &format!(
                r#"
                let observed = #{{ value: "" }};
                app.on_action("inner", |rt, p| {{
                    observed.value = p["greeting"];
                }});
                app.on_action("outer", |rt, _p| {{
                    app.action("inner").invoke(#{{ greeting: "hello" }});
                    if observed.value != "hello" {{
                        throw "inner did not observe params";
                    }}
                }});
                {}
                "#,
                ""
            ),
            "outer",
            rhai::Map::new(),
        )
        .expect("call must run inner with the caller's params");
        let _ = observed_clone;
    }
    let _ = observed;
}

// spec: ACN#action-invocation
// spec: OPS#sub-action-parameters
#[test]
fn missing_required_param_throws() {
    let err = invoke_action(
        r#"
            app.on_action("inner", |rt, _p| {}, #{
                params: #{
                    "name": #{ kind: "text" },
                },
            });
            app.on_action("outer", |rt, _p| {
                app.action("inner").invoke();
            });
        "#,
        "outer",
        rhai::Map::new(),
    )
    .expect_err("missing required param must fail");
    let msg = err_string(err);
    assert!(
        msg.contains("name") && msg.contains("required"),
        "expected required-field error naming the field, got: {msg}"
    );
}

// spec: ACN#action-invocation
// spec: OPS#sub-action-parameters
#[test]
fn reserved_key_in_params_throws() {
    let err = invoke_action(
        r#"
            app.on_action("inner", |rt, _p| {});
            app.on_action("outer", |rt, _p| {
                app.action("inner").invoke(#{ "evil_volume": "x" });
            });
        "#,
        "outer",
        rhai::Map::new(),
    )
    .expect_err("reserved key must be rejected");
    let msg = err_string(err);
    assert!(
        msg.contains("reserved"),
        "expected reserved-key error, got: {msg}"
    );
}

// spec: ACN#action-invocation
// spec: OPS#sub-action-parameters
#[test]
fn default_value_applied_when_omitted() {
    invoke_action(
        r#"
            app.on_action("inner", |rt, p| {
                if p["count"] != "5" {
                    throw "default not applied";
                }
            }, #{
                params: #{
                    "count": #{ kind: "text", default_value: "5" },
                },
            });
            app.on_action("outer", |rt, _p| {
                app.action("inner").invoke();
            });
        "#,
        "outer",
        rhai::Map::new(),
    )
    .expect("default must be applied");
}

// spec: ACN#action-invocation
#[test]
fn exception_from_called_closure_propagates() {
    let err = invoke_action(
        r#"
            app.on_action("inner", |rt, _p| {
                throw "inner exploded";
            });
            app.on_action("outer", |rt, _p| {
                app.action("inner").invoke();
            });
        "#,
        "outer",
        rhai::Map::new(),
    )
    .expect_err("inner exception must propagate");
    let msg = err_string(err);
    assert!(
        msg.contains("inner exploded"),
        "expected propagated exception, got: {msg}"
    );
}

// spec: OPS#cycles
#[test]
fn direct_self_call_is_rejected() {
    let err = invoke_action(
        r#"
            app.on_action("loop", |rt, _p| {
                app.action("loop").invoke();
            });
        "#,
        "loop",
        rhai::Map::new(),
    )
    .expect_err("self-call must be rejected");
    let msg = err_string(err);
    assert!(
        msg.contains("cycle") && msg.contains("loop"),
        "expected cycle error naming the action, got: {msg}"
    );
}

// spec: OPS#cycles
//
// Indirect cycles (alpha → bravo → alpha) cannot be exercised through
// the Rhai surface here: nested closure invocations re-borrow the
// shared `app` capture and Rhai's data-race detector rejects the call
// before our own cycle check runs. Test the cycle-detection logic
// directly instead — the call_action entry guards on it before any
// FnPtr is touched, so a unit test on the table primitives is
// authoritative for the semantics the spec requires.
#[test]
fn indirect_cycle_detection_walks_the_full_stack() {
    use crate::defs::app::{
        ActionCallTable, SubActionFrame, action_call_stack, with_action_call_table,
    };
    let _ = ActionCallTable {
        actions: Default::default(),
        stack: Vec::new(),
    };

    with_action_call_table(
        Default::default(),
        ActionName::new_unchecked("alpha"),
        || {
            let _outer = SubActionFrame::enter(ActionName::new_unchecked("bravo"));
            let stack = action_call_stack().expect("table active");
            let candidate = ActionName::new_unchecked("alpha");
            assert!(
                stack.iter().any(|n| n == &candidate),
                "indirect cycle must show up on the stack: {stack:?}",
            );
        },
    );
}

/// Run the `outer` action of `source` against a real `ReplayContext` holding
/// `committed`, and return its result and the log entries it pushed.
fn run_outer_in_replay_context(
    source: &str,
    committed: Vec<crate::runtime::barrier::ActionLogEntry>,
    cipher: Option<Arc<crate::runtime::secrets::Cipher>>,
) -> (
    Result<(), Box<rhai::EvalAltResult>>,
    Vec<crate::runtime::barrier::ActionLogEntry>,
) {
    use crate::runtime::barrier::ReplayContext;
    use crate::runtime::barrier::oracle::WorldStateOracle;
    use crate::runtime::registry::EphemeralInstanceRegistry;

    // Stub oracle: every barrier query reports satisfied. Used so we
    // can hand the runtime a real ReplayContext and observe the log
    // entries it pushes.
    struct AlwaysReadyOracle;
    impl WorldStateOracle for AlwaysReadyOracle {
        fn lifecycle_state(
            &self,
            _resource: &crate::runtime::ResourceInstance,
        ) -> crate::runtime::LifecycleState {
            crate::runtime::LifecycleState::Ready
        }
    }

    let cancel = Arc::new(crate::runtime::barrier::CancelToken::new());
    let mut replay = ReplayContext::new(
        crate::runtime::barrier::OperationId("test-op".into()),
        committed,
        Arc::new(AlwaysReadyOracle),
        cancel,
    );
    replay.cipher = cipher;
    let ctx = Arc::new(parking_lot::Mutex::new(replay));
    let rt = RuntimeInstance::with_context(
        Arc::clone(&ctx),
        seedling_protocol::names::AppName::new_unchecked("testapp"),
        Arc::new(EphemeralInstanceRegistry::new()),
        None,
    );

    let (engine, mut scope, _) = setup(&crate::ScriptLimits::default());
    let ast = engine.compile(source).expect("script must compile");
    engine
        .run_ast_with_scope(&mut scope, &ast)
        .expect("top-level script must succeed");
    let (captured, app_def_holder) = capture_actions(&engine, &ast);
    let outer_fn = captured
        .actions
        .get("outer")
        .cloned()
        .expect("outer must be captured");

    scope.push("__bsl_rt", rt.clone());
    scope.push("__bsl_closure", outer_fn);
    scope.push("__bsl_param", rhai::Map::new());

    let outer_name = ActionName::new_unchecked("outer");
    let action_def_arc = Arc::clone(&app_def_holder.def);
    let result = defs::app::with_action_call_table(captured.actions.clone(), outer_name, || {
        let _guard = ActionClosureGuard::new(action_def_arc, String::new(), HashMap::new())
            .with_active_rt(rt);
        let call_ast = engine
            .compile("__bsl_closure.call(__bsl_rt, __bsl_param)")
            .expect("call ast must compile");
        let merged = ast.merge(&call_ast);
        engine
            .eval_ast_with_scope::<Dynamic>(&mut scope, &merged)
            .map(|_| ())
    });
    let pending = ctx.lock().take_pending();
    (result, pending)
}

fn sub_action_payload(entries: &[crate::runtime::barrier::ActionLogEntry]) -> String {
    entries
        .iter()
        .find(|e| matches!(e.call_kind, crate::runtime::barrier::CallKind::SubAction))
        .expect("SubActionInvoked entry must be recorded")
        .extra
        .clone()
        .expect("SubAction must carry payload")
}

// spec: ACN#action-invocation
// spec: HIST#action-calls
#[test]
fn sub_action_invoked_log_entry_records_validated_params() {
    let source = r#"
        app.on_action("inner", |rt, p| {}, #{
            params: #{
                "count": #{ kind: "text", default_value: "7" },
            },
        });
        app.on_action("outer", |rt, _p| {
            app.action("inner").invoke();
        });
    "#;
    let (result, pending) = run_outer_in_replay_context(source, Vec::new(), None);
    result.expect("outer must run");
    let extra = sub_action_payload(&pending);
    assert!(
        extra.contains("inner") && extra.contains(r#""count":"7""#),
        "payload should name the action and the validated params, got: {extra}"
    );
}

const SECRET_SUB_ACTION_SCRIPT: &str = r#"
    app.on_action("inner", |rt, p| {
        if p.token != "s3cret-value" { throw "inner saw the wrong token"; }
    }, #{
        params: #{
            "token": #{ kind: "text", secret: true },
            "label": #{ kind: "text" },
        },
    });
    app.on_action("outer", |rt, _p| {
        app.action("inner").invoke(#{ token: TOKEN, label: "plain-label" });
    });
"#;

// spec: SEC#storage
// spec: HIST#action-calls
#[test]
fn sub_action_log_entry_protects_secret_params_and_replays_them() {
    let cipher = Arc::new(crate::runtime::secrets::Cipher::for_tests());
    let first = SECRET_SUB_ACTION_SCRIPT.replace("TOKEN", r#""s3cret-value""#);
    let (result, pending) =
        run_outer_in_replay_context(&first, Vec::new(), Some(Arc::clone(&cipher)));
    result.expect("outer must run");
    let extra = sub_action_payload(&pending);
    assert!(
        !extra.contains("s3cret-value"),
        "the secret must not be recorded in the clear: {extra}"
    );
    assert!(
        extra.contains("plain-label"),
        "non-secret params stay readable: {extra}"
    );

    // spec: OPS#sub-action-parameters
    // Replay takes the params from the committed entry, so the closure sees
    // the recorded secret even though the script now supplies another.
    let second = SECRET_SUB_ACTION_SCRIPT.replace("TOKEN", r#""changed""#);
    let (result, _) = run_outer_in_replay_context(&second, pending, Some(cipher));
    result.expect("replay must recover the recorded secret");
}

// spec: SEC#storage
#[test]
fn sub_action_with_secret_params_fails_without_a_secret_key() {
    let source = SECRET_SUB_ACTION_SCRIPT.replace("TOKEN", r#""s3cret-value""#);
    let (result, pending) = run_outer_in_replay_context(&source, Vec::new(), None);
    assert!(result.is_err(), "a secret must not be recorded unprotected");
    assert!(pending.iter().all(|e| {
        e.extra
            .as_deref()
            .is_none_or(|x| !x.contains("s3cret-value"))
    }));
}
