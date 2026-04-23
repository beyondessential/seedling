//! Handler probe — dry-run execution mode that enumerates container image
//! references an app's action/shell/install/on_change handlers might pull.
//!
//! See `r[image.discover]` in `docs/spec/runtime.md` for semantics. The
//! behaviour of individual `rt.*` calls under probe mode is implemented in
//! [`super::barrier::runtime`].

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

use parking_lot::Mutex;
use rhai::{AST, Dynamic, Engine, FnPtr, Scope};
use seedling_protocol::names::{ActionName, AppName, ParamName};

use crate::{
    defs::{
        app::{App, begin_closure_capture, end_closure_capture},
        install::ParamDef,
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
/// [`crate::defs::app::ClosureCapture`] plus the `start` handler, which is
/// captured as an action named `"start"` but surfaced specially.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerKind {
    Install,
    Start,
    Action,
    Shell,
    ParamChange,
}

impl HandlerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            HandlerKind::Install => "install",
            HandlerKind::Start => "start",
            HandlerKind::Action => "action",
            HandlerKind::Shell => "shell",
            HandlerKind::ParamChange => "param_change",
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
    /// Per-handler supplied param values. Outer key: handler name. Inner
    /// map: param name → string value.
    pub action_params: HashMap<String, HashMap<String, String>>,
    /// `true` → handlers with unresolved required params are _skipped_.
    /// `false` → the same condition is reported as an error.
    pub lenient: bool,
}

#[derive(Debug, Default, Clone)]
pub struct ProbeResponse {
    pub per_handler: Vec<HandlerProbe>,
    pub all_images: BTreeSet<String>,
}

// r[impl image.discover]
pub fn probe_app(
    engine: &Engine,
    script_ast: &AST,
    app: &App,
    registry: Arc<dyn InstanceRegistry>,
    request: &ProbeRequest,
) -> Result<ProbeResponse, String> {
    // Re-run the script to capture every registered closure. The script
    // itself must not fail; if it does, the caller can surface that as an
    // app-level script_error fault and there's nothing to probe.
    let app_name = app.def.load().name.clone();
    let captured = {
        let (mut scope, fresh_app) = crate::defs::scope();
        fresh_app.def.rcu(|d| {
            let mut d = (**d).clone();
            d.name = app_name.clone();
            d
        });
        *fresh_app.stored.lock() = app.stored.lock().clone();
        begin_closure_capture();
        let run_result = engine.run_ast_with_scope(&mut scope, script_ast);
        let captured = end_closure_capture();
        run_result.map_err(|e| format!("script evaluation failed: {e}"))?;
        captured
    };

    // Build the per-handler probe list in declaration order from the def.
    let def = app.def.load_full();
    let mut handlers: Vec<(String, HandlerKind, FnPtr, BTreeMap<ParamName, ParamDef>)> = Vec::new();

    if let Some(install_closure) = captured.install {
        let params = def
            .install
            .as_ref()
            .map(|i| i.requirements.clone())
            .unwrap_or_default();
        handlers.push((
            "install".to_owned(),
            HandlerKind::Install,
            install_closure,
            params,
        ));
    }

    for (name, closure) in &captured.actions {
        let kind = if name.as_str() == "start" {
            HandlerKind::Start
        } else {
            HandlerKind::Action
        };
        let params = def
            .actions
            .get(name)
            .map(|a| a.params.clone())
            .unwrap_or_default();
        handlers.push((name.as_str().to_owned(), kind, closure.clone(), params));
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

    for (name, closure) in &captured.param_changes {
        // on_change handlers take no params (they receive the old App).
        handlers.push((
            name.as_str().to_owned(),
            HandlerKind::ParamChange,
            closure.clone(),
            BTreeMap::new(),
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

#[allow(clippy::too_many_arguments)]
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
    // r[impl image.discover.params]
    let supplied = request.action_params.get(handler_name);
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
    let param_map: rhai::Map = resolved_params
        .iter()
        .map(|(k, v)| (k.as_str().into(), Dynamic::from(v.clone())))
        .collect();
    scope.push("__bsl_param", param_map);

    let call_script = match kind {
        HandlerKind::Shell => {
            scope.push(
                "__bsl_shell",
                crate::runtime::barrier::shell::ShellControl::new(),
            );
            "__bsl_closure.call(__bsl_rt, __bsl_shell, __bsl_param)"
        }
        HandlerKind::ParamChange => {
            // on_change receives an empty App in probe; dynamic refs that
            // depend on the previous generation's state are a best-effort
            // miss rather than a hard failure.
            scope.push("__bsl_old_app", App::default());
            "__bsl_closure.call(__bsl_rt, __bsl_old_app)"
        }
        _ => "__bsl_closure.call(__bsl_rt, __bsl_param)",
    };

    clear_barrier_hit();

    let action_def = Arc::new(arc_swap::ArcSwap::new(app.def.load_full()));
    let eval_result = {
        let _guard = ActionClosureGuard::new(action_def, op_id.0.clone(), HashMap::new());
        // r[impl image.discover]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs;
    use crate::runtime::EphemeralInstanceRegistry;
    use rhai::Engine;

    fn setup_engine_and_app(script: &str) -> (Engine, AST, App) {
        let mut engine = Engine::new();
        defs::register(&mut engine);
        let (mut scope, app) = defs::scope();
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

    // r[verify image.discover]
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
            probe_app(&engine, &ast, &app, registry, &ProbeRequest::default()).expect("probe");

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

    // r[verify image.discover]
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
            probe_app(&engine, &ast, &app, registry, &ProbeRequest::default()).expect("probe");

        assert!(
            response
                .all_images
                .contains("registry.example.com/api:2.0.0")
        );
    }

    // r[verify image.discover.params]
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
                        kind: crate::defs::install::ParamKind::Text,
                        required: true,
                        default_value: None,
                        description: None,
                        secret: false,
                    },
                );
            }
            d
        });

        let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
        let response = probe_app(
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

    // r[verify image.discover.params]
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
                        kind: crate::defs::install::ParamKind::Text,
                        required: true,
                        default_value: None,
                        description: None,
                        secret: false,
                    },
                );
            }
            d
        });

        let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
        let response = probe_app(
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

    // r[verify image.discover]
    #[test]
    fn handles_external_volume_with_unset_param_key() {
        // Mirrors the kopia-style backup handler:
        // `app.external_volume(param["output_volume"])` — `output_volume`
        // isn't in the handler's declared schema (it's supplied by the
        // runtime at real-invocation time), so in a probe it resolves to
        // unit. The probe should not throw; it should still extract
        // images from subsequent rt.start calls.
        let (engine, ast, app) = setup_engine_and_app(
            r#"
            app.on_action("list-snapshots", |rt, param| {
                let output = app.external_volume(param["output_volume"]);
                rt.start(
                    app.job()
                        .image("ghcr.io/example/kopia:0.21.0")
                        .mount("/output", output)
                ).ready();
            });
        "#,
        );

        let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
        let response =
            probe_app(&engine, &ast, &app, registry, &ProbeRequest::default()).expect("probe");

        let handler = response
            .per_handler
            .iter()
            .find(|h| h.name == "list-snapshots")
            .expect("list-snapshots probed");
        assert!(
            handler.error.is_none(),
            "expected clean probe, got error {:?}",
            handler.error
        );
        assert!(response.all_images.contains("ghcr.io/example/kopia:0.21.0"));
    }

    // r[verify image.discover]
    #[test]
    fn surfaces_error_for_nested_param_indexing() {
        // Kopia-like pattern: `backup["strategy"]` — `backup` is unset
        // during probe so indexing fails. This isn't recoverable by
        // probe stubs (we can't predict what structure the script
        // expects), but the error should be reported cleanly rather than
        // aborting the whole probe.
        let (engine, ast, app) = setup_engine_and_app(
            r#"
            app.on_action("list-snapshots", |rt, param| {
                let strategy = param["backup"]["strategy"];
                rt.start(app.job().image("ghcr.io/example/kopia:0.21.0"));
            });
            app.on_action("harmless", |rt, _param| {
                rt.start(app.job().image("ghcr.io/example/foo:1.0")).ready();
            });
        "#,
        );

        let registry: Arc<dyn InstanceRegistry> = Arc::new(EphemeralInstanceRegistry::new());
        let response = probe_app(
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
            .find(|h| h.name == "list-snapshots")
            .expect("probed");
        assert!(
            broken.error.is_some(),
            "expected error for nested param access"
        );
        // The clean handler still shows up in all_images.
        assert!(response.all_images.contains("ghcr.io/example/foo:1.0"));
        // The broken handler's image must not pollute all_images.
        assert!(!response.all_images.contains("ghcr.io/example/kopia:0.21.0"));
    }

    // r[verify image.discover]
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
            probe_app(&engine, &ast, &app, registry, &ProbeRequest::default()).expect("probe");

        assert!(response.all_images.contains("ghcr.io/example/step1:latest"));
        assert!(response.all_images.contains("ghcr.io/example/step2:latest"));
    }
}
