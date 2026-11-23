//! Handler probe — dry-run execution mode that enumerates container image
//! references an app's action and shell handlers and gate bodies might pull.
//!
//! See `spec: IMG#image-discovery` for semantics. The
//! behaviour of individual `rt.*` calls under probe mode is implemented in
//! [`super::barrier::runtime`].

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

use parking_lot::Mutex;
use rhai::{AST, Dynamic, Engine, FnPtr, Scope};
use seedling_protocol::names::{ActionName, AppName, GateName, ParamName};

use crate::{
    defs::{
        app::{App, capture_closures},
        param_def::ParamDef,
    },
    runtime::{
        InstanceRegistry,
        barrier::{
            CancelToken, OperationId, ReplayContext,
            oracle::TestWorldOracle,
            runtime::{ActionClosureGuard, ProbeGuard, RuntimeInstance, clear_barrier_hit},
        },
    },
};

/// Kind of handler being probed. Mirrors the shapes captured by
/// [`crate::defs::app::ClosureCapture`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerKind {
    Action,
    Shell,
    Gate,
}

impl HandlerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            HandlerKind::Action => "action",
            HandlerKind::Shell => "shell",
            HandlerKind::Gate => "gate",
        }
    }
}

/// Result of probing a single handler.
#[derive(Debug, Clone)]
pub struct HandlerProbe {
    pub name: String,
    pub kind: HandlerKind,
    pub images: BTreeSet<String>,
    /// Set when the closure threw or resource extraction failed.
    pub error: Option<String>,
    /// Set only in lenient mode when the probe elected not to invoke the
    /// closure. Mutually exclusive with `error`.
    pub skipped_reason: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct ProbeRequest {
    /// Per-handler supplied param values. Outer key: handler kind, as
    /// [`HandlerKind::as_str`] writes it. Next: handler name. Inner map: param
    /// name → string value.
    pub action_params: HashMap<String, HashMap<String, HashMap<String, String>>>,
    /// `true` → handlers with unresolved required params are _skipped_.
    /// `false` → the same condition is reported as an error.
    pub lenient: bool,
}

#[derive(Debug, Default, Clone)]
pub struct ProbeResponse {
    pub per_handler: Vec<HandlerProbe>,
    pub all_images: BTreeSet<String>,
}

/// The images the body of gate `gate` might pull in a run, as a probe of it
/// reports them, with `last` as the run would give it.
///
/// Nothing in the host or the app's persisted state changes.
// spec: IMG#image-discovery
// spec: GRUN#preparation
pub fn probe_gate_body(
    engine: &Engine,
    script_ast: &AST,
    app: &App,
    registry: Arc<dyn InstanceRegistry>,
    gate: &GateName,
    last: Option<&App>,
) -> Result<HandlerProbe, String> {
    let captured = capture_closures(engine, script_ast, app)
        .map_err(|e| format!("script evaluation failed: {e}"))?;
    match captured.gates.get(gate) {
        Some(Some(body)) => {
            probe_gate_closure(engine, script_ast, app, &registry, gate, body.clone(), last)
        }
        Some(None) => Ok(HandlerProbe {
            name: gate.as_str().to_owned(),
            kind: HandlerKind::Gate,
            images: BTreeSet::new(),
            error: None,
            skipped_reason: None,
        }),
        None => Err(format!("gate '{gate}' is not declared")),
    }
}

/// Probe the body `body` of gate `gate`.
fn probe_gate_closure(
    engine: &Engine,
    script_ast: &AST,
    app: &App,
    registry: &Arc<dyn InstanceRegistry>,
    gate: &GateName,
    body: FnPtr,
    last: Option<&App>,
) -> Result<HandlerProbe, String> {
    let arity = closure_arity(script_ast, &body)
        .ok_or_else(|| format!("the body of gate '{gate}' cannot be called"))?;
    Ok(run_probe(
        engine,
        app,
        registry,
        gate.as_str(),
        HandlerKind::Gate,
        body,
        ProbeCall::Gate {
            arity,
            last: last.map_or(Dynamic::UNIT, |last| Dynamic::from(last.clone())),
        },
    ))
}

/// `last_of` gives the app as it was at a gate's last success, `None` for a
/// gate with none, or why that could not be worked out, as a gate run would
/// give it to the body.
// spec: IMG#image-discovery
pub fn probe_app(
    engine: &Engine,
    script_ast: &AST,
    app: &App,
    registry: Arc<dyn InstanceRegistry>,
    request: &ProbeRequest,
    last_of: &dyn Fn(&GateName) -> Result<Option<App>, String>,
) -> Result<ProbeResponse, String> {
    // Re-run the script to capture every registered closure. The script
    // itself must not fail; if it does, the caller can surface that as an
    // app-level script_error fault and there's nothing to probe.
    let captured = capture_closures(engine, script_ast, app)
        .map_err(|e| format!("script evaluation failed: {e}"))?;

    // Build the per-handler probe list in declaration order from the def.
    let def = app.def.load_full();
    let mut handlers: Vec<(String, HandlerKind, FnPtr, BTreeMap<ParamName, ParamDef>)> = Vec::new();

    for (name, closure) in &captured.actions {
        let params = def
            .actions
            .get(name)
            .map(|a| a.params.clone())
            .unwrap_or_default();
        handlers.push((
            name.as_str().to_owned(),
            HandlerKind::Action,
            closure.clone(),
            params,
        ));
    }

    for (name, closure) in &captured.shells {
        let params = def
            .shells
            .get(name)
            .map(|s| s.params.clone())
            .unwrap_or_default();
        handlers.push((
            name.as_str().to_owned(),
            HandlerKind::Shell,
            closure.clone(),
            params,
        ));
    }

    let mut results = Vec::with_capacity(handlers.len());
    for (name, kind, closure, param_schema) in handlers {
        let probe_result = probe_handler(
            engine,
            app,
            &registry,
            &name,
            kind,
            closure,
            &param_schema,
            request,
        );
        results.push(probe_result);
    }

    // A gate without a body has nothing to probe.
    for (name, body) in &captured.gates {
        let Some(body) = body else { continue };
        let probed = last_of(name).and_then(|last| {
            probe_gate_closure(
                engine,
                script_ast,
                app,
                &registry,
                name,
                body.clone(),
                last.as_ref(),
            )
        });
        results.push(probed.unwrap_or_else(|error| HandlerProbe {
            name: name.as_str().to_owned(),
            kind: HandlerKind::Gate,
            images: BTreeSet::new(),
            error: Some(error),
            skipped_reason: None,
        }));
    }

    let mut all_images = BTreeSet::new();
    for r in &results {
        if r.error.is_none() && r.skipped_reason.is_none() {
            all_images.extend(r.images.iter().cloned());
        }
    }

    Ok(ProbeResponse {
        per_handler: results,
        all_images,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "the arguments are what one handler probe needs"
)]
fn probe_handler(
    engine: &Engine,
    app: &App,
    registry: &Arc<dyn InstanceRegistry>,
    handler_name: &str,
    kind: HandlerKind,
    closure: FnPtr,
    schema: &BTreeMap<ParamName, ParamDef>,
    request: &ProbeRequest,
) -> HandlerProbe {
    // spec: IMG#probe-parameters
    let supplied = request
        .action_params
        .get(kind.as_str())
        .and_then(|by_name| by_name.get(handler_name));
    let stored = app.stored.lock().clone();
    let (resolved_params, missing_required) = resolve_params(schema, supplied, &stored);

    if !missing_required.is_empty() {
        let msg = format!("requires params: {}", missing_required.join(", "));
        return HandlerProbe {
            name: handler_name.to_owned(),
            kind,
            images: BTreeSet::new(),
            error: if request.lenient {
                None
            } else {
                Some(msg.clone())
            },
            skipped_reason: if request.lenient { Some(msg) } else { None },
        };
    }

    let param_map: rhai::Map = resolved_params
        .iter()
        .map(|(k, v)| (k.as_str().into(), Dynamic::from(v.clone())))
        .collect();
    let call = match kind {
        HandlerKind::Shell => ProbeCall::Shell(param_map),
        HandlerKind::Action | HandlerKind::Gate => ProbeCall::Action(param_map),
    };
    run_probe(engine, app, registry, handler_name, kind, closure, call)
}

/// How a closure is called under a probe.
enum ProbeCall {
    Action(rhai::Map),
    Shell(rhai::Map),
    /// A gate's body, given as many of its arguments as it declares.
    Gate {
        arity: usize,
        last: Dynamic,
    },
}

/// Call `closure` under probe mode and report the images it reached.
fn run_probe(
    engine: &Engine,
    app: &App,
    registry: &Arc<dyn InstanceRegistry>,
    handler_name: &str,
    kind: HandlerKind,
    closure: FnPtr,
    call: ProbeCall,
) -> HandlerProbe {
    // Fresh probe buffer per handler. Shared across rt.* calls through the
    // ReplayContext and into Started's check_barrier.
    let images: Arc<Mutex<BTreeSet<String>>> = Arc::new(Mutex::new(BTreeSet::new()));
    let world = Arc::new(TestWorldOracle::new()) as Arc<dyn crate::runtime::WorldStateOracle>;
    let cancel_token = Arc::new(CancelToken::new());
    let op_id = OperationId::new();
    let ctx = Arc::new(Mutex::new(ReplayContext::new_probe(
        op_id.clone(),
        world,
        Arc::clone(&cancel_token),
        Arc::clone(&images),
    )));

    let app_name = app.def.load().name.clone();
    let rt = RuntimeInstance::with_context(
        Arc::clone(&ctx),
        app_name.clone(),
        Arc::clone(registry),
        None,
    );

    // Build scope the same way the live dispatcher does, then eval the
    // appropriate call script for this handler shape.
    let mut scope = Scope::new();
    // The script's top-level references `app`; recreate the variable.
    scope.push("app", app.clone());
    scope.push("__bsl_rt", rt);
    scope.push("__bsl_closure", closure);
    let call_script = match call {
        ProbeCall::Shell(params) => {
            scope.push("__bsl_param", params);
            scope.push(
                "__bsl_shell",
                crate::runtime::barrier::shell::ShellControl::new(),
            );
            "__bsl_closure.call(__bsl_rt, __bsl_shell, __bsl_param)"
        }
        ProbeCall::Action(params) => {
            scope.push("__bsl_param", params);
            "__bsl_closure.call(__bsl_rt, __bsl_param)"
        }
        ProbeCall::Gate { arity, last } => {
            scope.push("__bsl_last", last);
            match arity {
                0 => "__bsl_closure.call()",
                1 => "__bsl_closure.call(__bsl_rt)",
                _ => "__bsl_closure.call(__bsl_rt, __bsl_last)",
            }
        }
    };

    clear_barrier_hit();

    let action_def = Arc::new(arc_swap::ArcSwap::new(app.def.load_full()));
    let eval_result = {
        let mut _guard = ActionClosureGuard::new(action_def, op_id.0.clone(), HashMap::new());
        // A probe's derived values go nowhere, so any seed will do; with none
        // drawn, a closure that derives fails the probe of it.
        if let Ok(seed) = crate::runtime::seeds::Seed::draw() {
            _guard = _guard.with_operation_seed(&seed);
        }
        // spec: IMG#image-discovery
        let _probe_guard = ProbeGuard::new();
        engine.compile(call_script).ok().and_then(|ast| {
            engine
                .eval_ast_with_scope::<Dynamic>(&mut scope, &ast)
                .err()
        })
    };

    let gathered_images = images.lock().clone();

    let error = eval_result.map(|e| format!("{e}"));

    HandlerProbe {
        name: handler_name.to_owned(),
        kind,
        images: gathered_images,
        error,
        skipped_reason: None,
    }
}

/// Merge supplied, stored, and default-valued params. Returns the
/// resolved map and the list of required-but-missing names.
fn resolve_params(
    schema: &BTreeMap<ParamName, ParamDef>,
    supplied: Option<&HashMap<String, String>>,
    stored: &BTreeMap<String, String>,
) -> (BTreeMap<ParamName, String>, Vec<String>) {
    let mut resolved = BTreeMap::new();
    let mut missing = Vec::new();

    for (name, pdef) in schema {
        let value = supplied
            .and_then(|m| m.get(name.as_str()).cloned())
            .or_else(|| stored.get(name.as_str()).cloned())
            .or_else(|| pdef.default_value.clone());
        match value {
            Some(v) => {
                resolved.insert(name.clone(), v);
            }
            None if pdef.required => {
                missing.push(name.as_str().to_owned());
            }
            None => {}
        }
    }

    (resolved, missing)
}

/// How many parameters the closure `pointer` declares, or `None` where the
/// function it points to is not in `ast`.
pub(crate) fn closure_arity(ast: &AST, pointer: &FnPtr) -> Option<usize> {
    ast.iter_functions()
        .find(|function| function.name == pointer.fn_name())
        .map(|function| function.params.len().saturating_sub(pointer.curry().len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs;
    use crate::runtime::EphemeralInstanceRegistry;
    use rhai::Engine;

    fn setup_engine_and_app(script: &str) -> (Engine, AST, App) {
        let mut engine = Engine::new();
        defs::register(&mut engine);
        let (mut scope, app) = defs::scope(Default::default()).expect("the scope builds");
        app.def.rcu(|d| {
            let mut d = (**d).clone();
            d.name = AppName::new("testapp").unwrap();
            d
        });
        let ast = engine.compile(script).expect("compile");
        engine
            .run_ast_with_scope(&mut scope, &ast)
            .expect("run script");
        (engine, ast, app)
    }

    /// Probe `app` as it stands, where no gate has a last success.
    fn probe(
        engine: &Engine,
        ast: &AST,
        app: &App,
        registry: Arc<dyn InstanceRegistry>,
        request: &ProbeRequest,
    ) -> Result<ProbeResponse, String> {
        probe_app(engine, ast, app, registry, request, &|_| Ok(None))
    }

    // spec: IMG#image-discovery
    #[test]
    fn probes_dynamic_job_images_in_action() {
        let (engine, ast, app) = setup_engine_and_app(
            r#"
            app.on_action("preload", |rt, _param| {
                let j = app.job().image("ghcr.io/example/foo:1.2.3");
                rt.warm_images(j).ready();
            });
        "#,
        );

        let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
        let response =
            probe(&engine, &ast, &app, registry, &ProbeRequest::default()).expect("probe");

        let preload = response
            .per_handler
            .iter()
            .find(|h| h.name == "preload")
            .expect("preload probe");
        assert!(preload.error.is_none());
        assert!(preload.skipped_reason.is_none());
        assert!(
            preload.images.contains("ghcr.io/example/foo:1.2.3"),
            "expected image in probe output, got {:?}",
            preload.images
        );
        assert!(response.all_images.contains("ghcr.io/example/foo:1.2.3"));
    }

    const GATED: &str = r#"
        app.gate("bare").key(1);
        app.gate("migrate").key(1).run(|rt, last| {
            let image = if last == () { "ghcr.io/example/first:1" } else { "ghcr.io/example/again:1" };
            rt.warm_images(app.job().image(image)).ready();
        });
    "#;

    // spec: IMG#image-discovery
    #[test]
    fn probes_a_gate_body_with_last_as_a_run_would_give_it() {
        let (engine, ast, app) = setup_engine_and_app(GATED);
        let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
        let probed = |last: Option<App>| {
            probe_app(
                &engine,
                &ast,
                &app,
                Arc::clone(&registry),
                &ProbeRequest::default(),
                &move |_| Ok(last.clone()),
            )
            .expect("probe")
        };

        let first = probed(None);
        let [migrate] = first.per_handler.as_slice() else {
            panic!("only the gate with a body is probed: {first:?}");
        };
        assert_eq!(migrate.name, "migrate");
        assert_eq!(migrate.kind, HandlerKind::Gate);
        assert!(migrate.error.is_none(), "{migrate:?}");
        assert_eq!(
            migrate.images,
            BTreeSet::from(["ghcr.io/example/first:1".to_owned()]),
            "a gate with no last success is probed with the unit value"
        );
        assert!(first.all_images.contains("ghcr.io/example/first:1"));

        let (_, _, earlier) = setup_engine_and_app(GATED);
        let again = probed(Some(earlier));
        assert_eq!(
            again.per_handler[0].images,
            BTreeSet::from(["ghcr.io/example/again:1".to_owned()]),
            "a gate with a last success is probed with that app"
        );
    }

    // spec: IMG#image-discovery
    #[test]
    fn a_gate_whose_last_cannot_be_worked_out_is_reported_as_an_error_and_not_as_having_none() {
        let (engine, ast, app) = setup_engine_and_app(GATED);
        let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
        let response = probe_app(
            &engine,
            &ast,
            &app,
            registry,
            &ProbeRequest::default(),
            &|_| Err("generation 3 cannot be rebuilt".to_owned()),
        )
        .expect("probe");
        let migrate = &response.per_handler[0];
        assert_eq!(migrate.kind, HandlerKind::Gate);
        assert_eq!(
            migrate.error.as_deref(),
            Some("generation 3 cannot be rebuilt")
        );
        assert!(migrate.images.is_empty());
        assert!(response.all_images.is_empty(), "{response:?}");
    }

    // spec: IMG#image-discovery
    // spec: GATE#body
    #[test]
    fn a_gate_body_may_declare_no_parameter_or_one_under_a_probe() {
        for body in [
            "|| { app.job().image(\"ghcr.io/example/x:1\"); }",
            "|rt| { rt.warm_images(app.job().image(\"ghcr.io/example/x:1\")).ready(); }",
        ] {
            let (engine, ast, app) =
                setup_engine_and_app(&format!("app.gate(\"gated\").key(1).run({body});"));
            let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
            let response =
                probe(&engine, &ast, &app, registry, &ProbeRequest::default()).expect("probe");
            assert!(response.per_handler[0].error.is_none(), "{response:?}");
        }
    }

    // spec: IMG#image-discovery
    #[test]
    fn probes_rt_start_dynamic_deployment_image() {
        let (engine, ast, app) = setup_engine_and_app(
            r#"
            app.on_action("deploy", |rt, _param| {
                rt.start(app.deployment().image("registry.example.com/api:2.0.0")).ready();
            });
        "#,
        );

        let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
        let response =
            probe(&engine, &ast, &app, registry, &ProbeRequest::default()).expect("probe");

        assert!(
            response
                .all_images
                .contains("registry.example.com/api:2.0.0")
        );
    }

    // spec: IMG#image-discovery
    #[test]
    fn rt_query_records_no_images_during_a_probe() {
        let (engine, ast, app) = setup_engine_and_app(
            r#"
            app.on_action("look", |rt, _param| {
                rt.query(app.deployment().image("registry.example.com/queried:1.0")).ready();
                rt.start(app.deployment().image("registry.example.com/started:1.0")).ready();
            });
        "#,
        );

        let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
        let response =
            probe(&engine, &ast, &app, registry, &ProbeRequest::default()).expect("probe");

        assert!(
            response
                .all_images
                .contains("registry.example.com/started:1.0")
        );
        assert!(
            !response
                .all_images
                .contains("registry.example.com/queried:1.0")
        );
    }

    // spec: IMG#probe-parameters
    #[test]
    fn lenient_skips_action_with_missing_required_param() {
        let (engine, ast, app) = setup_engine_and_app(
            r#"
            let version = app.param("version").required(true);
            app.on_action("deploy", |rt, _param| {
                let d = app.deployment().image(`img:${version.value()}`);
                rt.start(d).ready();
            });
        "#,
        );

        // Add "version" as a declared param on the deploy action.
        app.def.rcu(|d| {
            let mut d = (**d).clone();
            if let Some(action) = d.actions.get_mut("deploy") {
                action.params.insert(
                    ParamName::new("version").unwrap(),
                    ParamDef {
                        kind: crate::defs::param_def::ParamKind::Text,
                        required: true,
                        default_value: None,
                        description: None,
                        secret: None,
                    },
                );
            }
            d
        });

        let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
        let response = probe(
            &engine,
            &ast,
            &app,
            registry,
            &ProbeRequest {
                action_params: HashMap::new(),
                lenient: true,
            },
        )
        .expect("probe");

        let deploy = response
            .per_handler
            .iter()
            .find(|h| h.name == "deploy")
            .expect("deploy probe");
        assert!(deploy.skipped_reason.is_some());
        assert!(deploy.error.is_none());
        assert!(
            !response
                .all_images
                .contains("registry.example.com/api:2.0.0")
        );
    }

    // spec: IMG#probe-parameters
    #[test]
    fn supplied_values_are_keyed_by_handler_kind_then_name() {
        let (engine, ast, app) = setup_engine_and_app(
            r#"
            app.on_action("deploy", |rt, param| {
                rt.start(app.deployment().image(`registry.example.com/api:${param["version"]}`)).ready();
            });
        "#,
        );
        app.def.rcu(|d| {
            let mut d = (**d).clone();
            if let Some(action) = d.actions.get_mut("deploy") {
                action.params.insert(
                    ParamName::new("version").unwrap(),
                    ParamDef {
                        kind: crate::defs::param_def::ParamKind::Text,
                        required: true,
                        default_value: None,
                        description: None,
                        secret: None,
                    },
                );
            }
            d
        });
        let supplied = |kind: &str| ProbeRequest {
            action_params: HashMap::from([(
                kind.to_owned(),
                HashMap::from([(
                    "deploy".to_owned(),
                    HashMap::from([("version".to_owned(), "1.2".to_owned())]),
                )]),
            )]),
            lenient: false,
        };

        let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
        let response = probe(
            &engine,
            &ast,
            &app,
            Arc::clone(&registry),
            &supplied("action"),
        )
        .expect("probe");
        assert!(
            response.all_images.contains("registry.example.com/api:1.2"),
            "{response:?}"
        );

        let response = probe(&engine, &ast, &app, registry, &supplied("shell")).expect("probe");
        let deploy = &response.per_handler[0];
        assert!(
            deploy.error.is_some(),
            "a value filed under another kind must not reach the action"
        );
    }

    // spec: IMG#probe-parameters
    #[test]
    fn strict_reports_missing_required_param_as_error() {
        let (engine, ast, app) = setup_engine_and_app(
            r#"
            app.on_action("deploy", |rt, _param| {});
        "#,
        );
        app.def.rcu(|d| {
            let mut d = (**d).clone();
            if let Some(action) = d.actions.get_mut("deploy") {
                action.params.insert(
                    ParamName::new("xyz").unwrap(),
                    ParamDef {
                        kind: crate::defs::param_def::ParamKind::Text,
                        required: true,
                        default_value: None,
                        description: None,
                        secret: None,
                    },
                );
            }
            d
        });

        let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
        let response = probe(
            &engine,
            &ast,
            &app,
            registry,
            &ProbeRequest {
                action_params: HashMap::new(),
                lenient: false,
            },
        )
        .expect("probe");

        let deploy = response
            .per_handler
            .iter()
            .find(|h| h.name == "deploy")
            .unwrap();
        assert!(deploy.error.is_some());
        assert!(deploy.skipped_reason.is_none());
    }

    // spec: IMG#image-discovery
    #[test]
    fn handles_external_volume_with_unset_param_key() {
        // A handler that calls `app.external_volume(param["output_volume"])`:
        // `output_volume`
        // isn't in the handler's declared schema (it's supplied by the
        // runtime at real-invocation time), so in a probe it resolves to
        // unit. The probe should not throw; it should still extract
        // images from subsequent rt.start calls.
        let (engine, ast, app) = setup_engine_and_app(
            r#"
            app.on_action("export", |rt, param| {
                let output = app.external_volume(param["output_volume"]);
                rt.start(
                    app.job()
                        .image("ghcr.io/example/tool:0.21.0")
                        .mount("/output", output)
                ).ready();
            });
        "#,
        );

        let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
        let response =
            probe(&engine, &ast, &app, registry, &ProbeRequest::default()).expect("probe");

        let handler = response
            .per_handler
            .iter()
            .find(|h| h.name == "export")
            .expect("export probed");
        assert!(
            handler.error.is_none(),
            "expected clean probe, got error {:?}",
            handler.error
        );
        assert!(response.all_images.contains("ghcr.io/example/tool:0.21.0"));
    }

    // spec: IMG#image-discovery
    #[test]
    fn surfaces_error_for_nested_param_indexing() {
        // Nested indexing such as `config["mode"]` — `config` is unset
        // during probe so indexing fails. This isn't recoverable by
        // probe stubs (we can't predict what structure the script
        // expects), but the error should be reported cleanly rather than
        // aborting the whole probe.
        let (engine, ast, app) = setup_engine_and_app(
            r#"
            app.on_action("export", |rt, param| {
                let mode = param["config"]["mode"];
                rt.start(app.job().image("ghcr.io/example/tool:0.21.0"));
            });
            app.on_action("harmless", |rt, _param| {
                rt.start(app.job().image("ghcr.io/example/foo:1.0")).ready();
            });
        "#,
        );

        let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
        let response = probe(
            &engine,
            &ast,
            &app,
            registry,
            &ProbeRequest {
                lenient: true,
                ..Default::default()
            },
        )
        .expect("probe");

        let broken = response
            .per_handler
            .iter()
            .find(|h| h.name == "export")
            .expect("probed");
        assert!(
            broken.error.is_some(),
            "expected error for nested param access"
        );
        // The clean handler still shows up in all_images.
        assert!(response.all_images.contains("ghcr.io/example/foo:1.0"));
        // The broken handler's image must not pollute all_images.
        assert!(!response.all_images.contains("ghcr.io/example/tool:0.21.0"));
    }

    // spec: IMG#image-discovery
    #[test]
    fn passes_through_termination_ensure_success() {
        // A closure that gates further image refs on a terminated job's
        // ensure_success should see that pass in probe mode.
        let (engine, ast, app) = setup_engine_and_app(
            r#"
            app.on_action("chain", |rt, _param| {
                let first = rt.start(
                    app.job().image("ghcr.io/example/step1:latest")
                );
                first.terminated().ensure_success();
                let second = app.job().image("ghcr.io/example/step2:latest");
                rt.start(second).ready();
            });
        "#,
        );

        let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
        let response =
            probe(&engine, &ast, &app, registry, &ProbeRequest::default()).expect("probe");

        assert!(response.all_images.contains("ghcr.io/example/step1:latest"));
        assert!(response.all_images.contains("ghcr.io/example/step2:latest"));
    }
}
