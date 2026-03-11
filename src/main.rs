use std::path::PathBuf;

use rhai::{AST, Dynamic, Engine, Scope};

mod defs;
use defs::install::InstallDef;

fn setup() -> (Engine, Scope<'static>, defs::app::App) {
    let mut engine = Engine::new();
    defs::register(&mut engine);
    let (scope, app) = defs::scope();
    (engine, scope, app)
}

fn exercise_actions(engine: &Engine, scope: &mut Scope, app: &defs::app::App, script_ast: &AST) {
    let def = app.0.lock();

    let rt = defs::runtime::RuntimeInstance;
    let attach = defs::runtime::shell_attach_fn_ptr();
    let history = defs::history::History;
    let old_app = defs::app::App::default();

    let actions: Vec<_> = def
        .actions
        .iter()
        .map(|(name, a)| (name.clone(), a.closure.clone()))
        .collect();
    let shells: Vec<_> = def
        .shells
        .iter()
        .map(|(name, s)| (name.clone(), s.closure.clone()))
        .collect();
    let install = def.install.as_ref().map(|i| {
        let reqs_map = build_install_reqs_map(i);
        (i.closure.clone(), reqs_map)
    });

    drop(def);

    for (name, closure) in &actions {
        scope.push("__bsl_rt", rt.clone());
        scope.push("__bsl_closure", closure.clone());
        scope.push("__bsl_old_app", old_app.clone());
        scope.push("__bsl_history", history.clone());

        let call_script = match name.as_str() {
            "upgrade" => "__bsl_closure.call(__bsl_rt, __bsl_old_app)",
            "crash_recovery" => "__bsl_closure.call(__bsl_rt, __bsl_history)",
            _ => "__bsl_closure.call(__bsl_rt)",
        };

        println!("exercising action: {name}");
        match eval_merged(engine, scope, script_ast, call_script) {
            Ok(_) => println!("  ok"),
            Err(err) => println!("  error: {err}"),
        }

        let _ = scope.remove::<Dynamic>("__bsl_rt");
        let _ = scope.remove::<Dynamic>("__bsl_closure");
        let _ = scope.remove::<Dynamic>("__bsl_old_app");
        let _ = scope.remove::<Dynamic>("__bsl_history");
    }

    for (name, closure) in &shells {
        scope.push("__bsl_rt", rt.clone());
        scope.push("__bsl_closure", closure.clone());
        scope.push("__bsl_attach", attach.clone());

        println!("exercising shell: {name}");
        let two_arg = "__bsl_closure.call(__bsl_rt, __bsl_attach)";
        let one_arg = "__bsl_closure.call(__bsl_rt)";
        match eval_merged(engine, scope, script_ast, two_arg) {
            Ok(_) => println!("  ok (two-arg)"),
            Err(err_two) => match eval_merged(engine, scope, script_ast, one_arg) {
                Ok(_) => println!("  ok (one-arg)"),
                Err(err_one) => {
                    println!("  error (two-arg): {err_two}");
                    println!("  error (one-arg): {err_one}");
                }
            },
        }

        let _ = scope.remove::<Dynamic>("__bsl_rt");
        let _ = scope.remove::<Dynamic>("__bsl_closure");
        let _ = scope.remove::<Dynamic>("__bsl_attach");
    }

    if let Some((closure, reqs_map)) = &install {
        scope.push("__bsl_rt", rt.clone());
        scope.push("__bsl_closure", closure.clone());
        scope.push("__bsl_reqs", reqs_map.clone());

        println!("exercising install");
        let call_script = "__bsl_closure.call(__bsl_rt, __bsl_reqs)";
        match eval_merged(engine, scope, script_ast, call_script) {
            Ok(_) => println!("  ok"),
            Err(err) => println!("  error: {err}"),
        }

        let _ = scope.remove::<Dynamic>("__bsl_rt");
        let _ = scope.remove::<Dynamic>("__bsl_closure");
        let _ = scope.remove::<Dynamic>("__bsl_reqs");
    }
}

fn eval_merged(
    engine: &Engine,
    scope: &mut Scope,
    script_ast: &AST,
    call_source: &str,
) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
    let call_ast = engine.compile(call_source)?;
    let merged = script_ast.merge(&call_ast);
    engine.eval_ast_with_scope(scope, &merged)
}

fn build_install_reqs_map(install: &InstallDef) -> rhai::Map {
    let mut map = rhai::Map::new();
    for (key, req) in &install.requirements {
        let value = req
            .default_value
            .clone()
            .unwrap_or_else(|| "<placeholder>".into());
        map.insert(key.as_str().into(), Dynamic::from(value));
    }
    map
}

fn run_script(
    engine: &Engine,
    scope: &mut Scope,
    source: &str,
) -> Result<AST, Box<rhai::EvalAltResult>> {
    let ast = engine.compile(source)?;
    engine.run_ast_with_scope(scope, &ast)?;
    Ok(ast)
}

fn run_file(
    engine: &Engine,
    scope: &mut Scope,
    path: PathBuf,
) -> Result<AST, Box<rhai::EvalAltResult>> {
    let ast = engine.compile_file(path)?;
    engine.run_ast_with_scope(scope, &ast)?;
    Ok(ast)
}

fn main() {
    let filepath = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("Usage: beset <RHAI FILE>"),
    );

    let (engine, mut scope, app) = setup();

    let ast = match run_file(&engine, &mut scope, filepath) {
        Ok(ast) => ast,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };

    let def = app.0.lock();
    println!("params: {:?}", def.params.keys().collect::<Vec<_>>());
    println!("resources: {}", def.resources.len());
    for id in def.resources.keys() {
        println!("  {:?} {:?}", id.kind, id.name);
    }
    println!("actions: {:?}", def.actions.keys().collect::<Vec<_>>());
    println!("shells: {:?}", def.shells.keys().collect::<Vec<_>>());
    println!("install: {}", def.install.is_some());
    drop(def);

    println!();
    println!("--- exercising actions ---");
    exercise_actions(&engine, &mut scope, &app, &ast);
}

#[cfg(test)]
mod tests {
    use super::*;
    use defs::resource::ResourceKind;

    fn run_test_script(source: &str) -> (Engine, Scope<'static>, defs::app::App, AST) {
        let (engine, mut scope, app) = setup();
        let ast = run_script(&engine, &mut scope, source).expect("script should run without error");
        (engine, scope, app, ast)
    }

    fn run_test_script_app(source: &str) -> defs::app::App {
        let (_, _, app, _) = run_test_script(source);
        app
    }

    // l[verify param.type]
    // l[verify bsl.placeholder]
    #[test]
    fn param_returns_placeholder() {
        let app = run_test_script_app(r#"let x = app.param("foo");"#);
        let def = app.0.lock();
        assert!(def.params.contains_key("foo"));
        assert_eq!(def.params["foo"], "<placeholder>");
    }

    // l[verify service.type]
    // l[verify app.resources]
    #[test]
    fn service_creates_resource() {
        let app = run_test_script_app(r#"let s = app.service("web");"#);
        let def = app.0.lock();
        assert!(
            def.resources
                .keys()
                .any(|id| id.kind == ResourceKind::Service && &*id.name == "web")
        );
    }

    // l[verify app.resources.names]
    #[test]
    fn service_same_name_returns_same_resource() {
        let app = run_test_script_app(
            r#"
            let a = app.service("data");
            let b = app.service("data");
        "#,
        );
        let def = app.0.lock();
        let count = def
            .resources
            .keys()
            .filter(|id| id.kind == ResourceKind::Service && &*id.name == "data")
            .count();
        assert_eq!(count, 1);
    }

    // l[verify service.http]
    #[test]
    fn service_http_specialisation() {
        let app = run_test_script_app(
            r#"
            let h = app.service("api").http(8080);
        "#,
        );
        let def = app.0.lock();
        let id = def
            .resources
            .keys()
            .find(|id| id.kind == ResourceKind::Service && &*id.name == "api")
            .unwrap();
        if let defs::resource::Resource::Service(svc) = &def.resources[id] {
            assert!(svc.def.lock().http.is_some());
        } else {
            panic!("expected Service");
        }
    }

    // l[verify deployment.type]
    // l[verify container.image]
    // l[verify deployment.scale]
    #[test]
    fn deployment_with_image_and_scale() {
        let app = run_test_script_app(
            r#"
            app.deployment("web")
                .image("nginx:latest")
                .scale(3);
        "#,
        );
        let def = app.0.lock();
        let id = def
            .resources
            .keys()
            .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
            .unwrap();
        if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
            let dep_def = dep.def.lock();
            assert_eq!(dep_def.scale, 3..3);
            let pod = dep_def.pod.lock();
            let container = pod.container.lock();
            assert_eq!(container.image.as_deref(), Some("nginx:latest"));
        } else {
            panic!("expected Deployment");
        }
    }

    // l[verify deployment.scale]
    #[test]
    fn deployment_scale_range() {
        let app = run_test_script_app(
            r#"
            app.deployment("workers")
                .scale(1..8);
        "#,
        );
        let def = app.0.lock();
        let id = def
            .resources
            .keys()
            .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "workers")
            .unwrap();
        if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
            let dep_def = dep.def.lock();
            assert_eq!(dep_def.scale, 1..8);
        } else {
            panic!("expected Deployment");
        }
    }

    // l[verify deployment.on-update]
    // l[verify const.on-update.replace]
    #[test]
    fn deployment_on_update_replace() {
        let app = run_test_script_app(
            r#"
            app.deployment("sync")
                .on_update(OnUpdate.Replace);
        "#,
        );
        let def = app.0.lock();
        let id = def
            .resources
            .keys()
            .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "sync")
            .unwrap();
        if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
            let dep_def = dep.def.lock();
            assert!(matches!(dep_def.on_update, defs::enums::OnUpdate::Replace));
        } else {
            panic!("expected Deployment");
        }
    }

    // l[verify job.type]
    // l[verify job.deadline]
    // l[verify container.command]
    #[test]
    fn job_with_deadline() {
        let app = run_test_script_app(
            r#"
            app.job("migrate")
                .image("db-tools:1.0")
                .command("migrate")
                .deadline(300);
        "#,
        );
        let def = app.0.lock();
        let id = def
            .resources
            .keys()
            .find(|id| id.kind == ResourceKind::Job && &*id.name == "migrate")
            .unwrap();
        if let defs::resource::Resource::Job(job) = &def.resources[id] {
            let job_def = job.def.lock();
            assert_eq!(job_def.deadline, Some(300));
        } else {
            panic!("expected Job");
        }
    }

    // l[verify volume.type]
    #[test]
    fn volume_named() {
        let app = run_test_script_app(
            r#"
            let v = app.volume("data");
        "#,
        );
        let def = app.0.lock();
        assert!(
            def.resources
                .keys()
                .any(|id| id.kind == ResourceKind::Volume && &*id.name == "data")
        );
    }

    // l[verify volume.write]
    #[test]
    fn volume_write() {
        let app = run_test_script_app(
            r#"
            let v = app.volume("cfg");
            v.write("/app.conf", "key=value");
        "#,
        );
        let def = app.0.lock();
        let id = def
            .resources
            .keys()
            .find(|id| id.kind == ResourceKind::Volume && &*id.name == "cfg")
            .unwrap();
        if let defs::resource::Resource::Volume(vol) = &def.resources[id] {
            let vol_def = vol.def.lock();
            assert_eq!(vol_def.writes.len(), 1);
            assert_eq!(vol_def.writes[0], ("/app.conf".into(), "key=value".into()));
        } else {
            panic!("expected Volume");
        }
    }

    // l[verify volume.external]
    #[test]
    fn external_volume_creates_resource() {
        let app = run_test_script_app(
            r#"
            let v = app.external_volume("pg-socket");
        "#,
        );
        let def = app.0.lock();
        assert!(
            def.resources
                .keys()
                .any(|id| id.kind == ResourceKind::ExternalVolume && &*id.name == "pg-socket")
        );
    }

    // l[verify service.external]
    #[test]
    fn external_service_creates_resource() {
        let app = run_test_script_app(
            r#"
            let s = app.external_service("redis");
        "#,
        );
        let def = app.0.lock();
        assert!(
            def.resources
                .keys()
                .any(|id| id.kind == ResourceKind::ExternalService && &*id.name == "redis")
        );
    }

    // l[verify action.type]
    // l[verify action.option-description]
    #[test]
    fn on_action_registers_and_returns_action() {
        let app = run_test_script_app(
            r#"
            let a = app.on_action("migrate", |rt| {}, #{
                description: "Run migrations",
            });
        "#,
        );
        let def = app.0.lock();
        let action = def.actions.get("migrate").expect("action should exist");
        assert_eq!(action.description.as_deref(), Some("Run migrations"));
    }

    // l[verify action.start]
    #[test]
    fn on_start_registers_start_action() {
        let app = run_test_script_app(
            r#"
            app.on_start(|rt| {});
        "#,
        );
        let def = app.0.lock();
        assert!(def.actions.contains_key("start"));
    }

    // l[verify action.upgrade]
    #[test]
    fn on_upgrade_registers_upgrade_action() {
        let app = run_test_script_app(
            r#"
            app.on_upgrade(|rt, old| {});
        "#,
        );
        let def = app.0.lock();
        assert!(def.actions.contains_key("upgrade"));
    }

    // l[verify action.crash-recovery]
    #[test]
    fn on_crash_recovery_registers() {
        let app = run_test_script_app(
            r#"
            app.on_crash_recovery(|rt, history| {});
        "#,
        );
        let def = app.0.lock();
        assert!(def.actions.contains_key("crash_recovery"));
    }

    // l[verify action.shell]
    #[test]
    fn on_shell_registers_shell() {
        let app = run_test_script_app(
            r#"
            app.on_shell("node", |rt| {
                app.job("shell-node").image("node:20").command("node")
            }, #{
                description: "Node REPL",
            });
        "#,
        );
        let def = app.0.lock();
        let shell = def.shells.get("node").expect("shell should exist");
        assert_eq!(shell.description.as_deref(), Some("Node REPL"));
    }

    // l[verify action.install]
    // l[verify action.install.requirements]
    // l[verify action.install.requirements.kind-email]
    // l[verify action.install.requirements.kind-password]
    #[test]
    fn on_install_with_requirements() {
        let app = run_test_script_app(
            r#"
            app.on_install(|rt, reqs| {}, #{
                admin_email: #{
                    kind: "email",
                    description: "Admin email",
                    default_value: "admin@example.com",
                },
                admin_password: #{
                    kind: "password",
                    description: "Admin password",
                },
            });
        "#,
        );
        let def = app.0.lock();
        let install = def.install.as_ref().expect("install should exist");
        assert_eq!(install.requirements.len(), 2);
        let email_req = &install.requirements["admin_email"];
        assert!(matches!(
            email_req.kind,
            defs::install::InstallRequirementKind::Email
        ));
        assert_eq!(
            email_req.default_value.as_deref(),
            Some("admin@example.com")
        );
        let pw_req = &install.requirements["admin_password"];
        assert!(matches!(
            pw_req.kind,
            defs::install::InstallRequirementKind::Password
        ));
        assert!(pw_req.default_value.is_none());
    }

    // l[verify ingress.type]
    // l[verify ingress.http]
    // l[verify ingress.service]
    #[test]
    fn ingress_builder_chain() {
        let app = run_test_script_app(
            r#"
            let domain = "example.com";
            let traffic = app.service("public")
                .ingress(domain, 443).http()
                .service()
                .http(80);
        "#,
        );
        let def = app.0.lock();
        assert!(
            def.resources
                .keys()
                .any(|id| id.kind == ResourceKind::Service && &*id.name == "public")
        );
    }

    // l[verify container.env]
    #[test]
    fn container_env_override() {
        let app = run_test_script_app(
            r#"
            app.deployment("web")
                .image("app:1")
                .env("KEY", "old")
                .env("KEY", "new");
        "#,
        );
        let def = app.0.lock();
        let id = def
            .resources
            .keys()
            .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
            .unwrap();
        if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
            let dep_def = dep.def.lock();
            let pod = dep_def.pod.lock();
            let container = pod.container.lock();
            assert_eq!(container.env.len(), 1);
            assert_eq!(container.env[0], ("KEY".into(), "new".into()));
        } else {
            panic!("expected Deployment");
        }
    }

    // l[verify container.command]
    #[test]
    fn container_command_array() {
        let app = run_test_script_app(
            r#"
            app.job("task")
                .command(["bash", "-c", "echo hello"]);
        "#,
        );
        let def = app.0.lock();
        let id = def
            .resources
            .keys()
            .find(|id| id.kind == ResourceKind::Job && &*id.name == "task")
            .unwrap();
        if let defs::resource::Resource::Job(job) = &def.resources[id] {
            let job_def = job.def.lock();
            let pod = job_def.pod.lock();
            let container = pod.container.lock();
            assert_eq!(
                container.command.as_deref(),
                Some(&["bash".to_string(), "-c".into(), "echo hello".into()][..])
            );
        } else {
            panic!("expected Job");
        }
    }

    // l[verify pod.tcp]
    #[test]
    fn pod_tcp_binding() {
        let app = run_test_script_app(
            r#"
            let svc = app.service("ctrl");
            app.deployment("web")
                .tcp(8080, svc);
        "#,
        );
        let def = app.0.lock();
        let id = def
            .resources
            .keys()
            .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
            .unwrap();
        if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
            let dep_def = dep.def.lock();
            let pod = dep_def.pod.lock();
            assert_eq!(pod.tcp_bindings.len(), 1);
            assert_eq!(pod.tcp_bindings[0].pod_port, 8080);
        } else {
            panic!("expected Deployment");
        }
    }

    // l[verify pod.http]
    // l[verify service.http.route]
    #[test]
    fn pod_http_route_binding() {
        let app = run_test_script_app(
            r#"
            let traffic = app.service("public").http(80);
            app.deployment("api")
                .http(3000, traffic.route("/api"));
        "#,
        );
        let def = app.0.lock();
        let id = def
            .resources
            .keys()
            .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "api")
            .unwrap();
        if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
            let dep_def = dep.def.lock();
            let pod = dep_def.pod.lock();
            assert_eq!(pod.http_bindings.len(), 1);
            assert_eq!(pod.http_bindings[0].pod_port, 3000);
            assert_eq!(pod.http_bindings[0].route.prefix, "/api");
        } else {
            panic!("expected Deployment");
        }
    }

    // l[verify const.available-threads]
    #[test]
    fn available_threads_constant() {
        run_test_script_app(
            r#"
            app.deployment("workers")
                .scale(1..AVAILABLE_THREADS);
        "#,
        );
    }

    // l[verify app.resources.dynamic]
    #[test]
    fn closures_create_dynamic_resources() {
        let app = run_test_script_app(
            r#"
            let make_job = || app.job("ephemeral")
                .image("tools:1")
                .command("run");

            app.on_start(|rt| {
                let j = make_job.call();
                rt.start(j).terminated();
            });
        "#,
        );
        let def = app.0.lock();
        assert!(def.actions.contains_key("start"));
    }

    // --- exercise tests: these actually invoke the action closures ---

    // l[verify rt.start]
    // l[verify rt.started.state-methods]
    #[test]
    fn exercise_start_action() {
        let (engine, mut scope, app, ast) = run_test_script(
            r#"
            app.on_start(|rt| {
                let svc = app.service("web");
                let dep = app.deployment("web").image("nginx");
                rt.start(svc);
                rt.start(dep).ready();
            });
        "#,
        );
        exercise_actions(&engine, &mut scope, &app, &ast);
    }

    // l[verify rt.started.terminated]
    // l[verify rt.termination.ensure-success]
    #[test]
    fn exercise_terminated_ensure_success() {
        let (engine, mut scope, app, ast) = run_test_script(
            r#"
            app.on_start(|rt| {
                let job = app.job("init").image("tools").command("setup");
                rt.start(job).terminated().ensure_success();
            });
        "#,
        );
        exercise_actions(&engine, &mut scope, &app, &ast);
    }

    // l[verify rt.stop]
    #[test]
    fn exercise_stop() {
        let (engine, mut scope, app, ast) = run_test_script(
            r#"
            app.on_start(|rt| {
                let dep = app.deployment("web").image("nginx");
                let started = rt.start(dep);
                started.ready();
                rt.stop(dep);
            });
        "#,
        );
        exercise_actions(&engine, &mut scope, &app, &ast);
    }

    // l[verify rt.query]
    #[test]
    fn exercise_query() {
        let (engine, mut scope, app, ast) = run_test_script(
            r#"
            app.on_start(|rt| {
                let dep = app.deployment("web").image("nginx");
                let queried = rt.query(dep);
                queried.ready();
            });
        "#,
        );
        exercise_actions(&engine, &mut scope, &app, &ast);
    }

    // l[verify rt.reconcile]
    #[test]
    fn exercise_reconcile() {
        let (engine, mut scope, app, ast) = run_test_script(
            r#"
            app.on_upgrade(|rt, old| {
                let svc = app.service("public");
                rt.reconcile(old, svc);
            });
        "#,
        );
        exercise_actions(&engine, &mut scope, &app, &ast);
    }

    // l[verify action.upgrade]
    #[test]
    fn exercise_upgrade_action() {
        let (engine, mut scope, app, ast) = run_test_script(
            r#"
            app.on_upgrade(|rt, old| {
                rt.start(app.deployment("web").image("nginx:2"));
                rt.stop(old);
            });
        "#,
        );
        exercise_actions(&engine, &mut scope, &app, &ast);
    }

    // l[verify action.crash-recovery]
    // l[verify history.was-upgrading]
    #[test]
    fn exercise_crash_recovery_action() {
        let (engine, mut scope, app, ast) = run_test_script(
            r#"
            app.on_crash_recovery(|rt, history| {
                if history.was_upgrading() {
                    rt.start(app.job("fixup").image("tools").command("repair")).terminated();
                }
                rt.start(app.deployment("web").image("nginx"));
            });
        "#,
        );
        exercise_actions(&engine, &mut scope, &app, &ast);
    }

    // l[verify action.shell]
    // l[verify action.shell.attach]
    #[test]
    fn exercise_shell_with_attach() {
        let (engine, mut scope, app, ast) = run_test_script(
            r#"
            app.on_shell("db", |rt, attach| {
                let shell = app.job("shell-db")
                    .image("tools")
                    .command("psql");
                rt.start(shell).running();
                attach.call(shell);
            });
        "#,
        );
        exercise_actions(&engine, &mut scope, &app, &ast);
    }

    // l[verify action.shell]
    #[test]
    fn exercise_shell_return_job() {
        let (engine, mut scope, app, ast) = run_test_script(
            r#"
            app.on_shell("node", |rt| {
                app.job("shell-node").image("node:20").command("node")
            });
        "#,
        );
        exercise_actions(&engine, &mut scope, &app, &ast);
    }

    // l[verify action.install]
    // l[verify action.install.requirements]
    #[test]
    fn exercise_install_action() {
        let (engine, mut scope, app, ast) = run_test_script(
            r#"
            app.on_install(|rt, reqs| {
                rt.start(app.deployment("web").image("nginx")).ready();
            }, #{
                admin_email: #{
                    kind: "email",
                    description: "Admin email",
                },
            });
        "#,
        );
        exercise_actions(&engine, &mut scope, &app, &ast);
    }

    // l[verify bsl.script]
    #[test]
    fn draft_script_runs_and_exercises() {
        let (engine, mut scope, app) = setup();
        let ast = run_file(
            &engine,
            &mut scope,
            std::path::PathBuf::from("draft-beset.rhai"),
        )
        .expect("draft script should run");

        let def = app.0.lock();
        assert!(def.params.contains_key("domain"));
        assert!(def.params.contains_key("version"));
        assert!(def.actions.contains_key("start"));
        assert!(def.actions.contains_key("upgrade"));
        assert!(def.actions.contains_key("crash_recovery"));
        assert!(def.actions.contains_key("migrate"));
        assert!(def.shells.contains_key("node"));
        assert!(def.shells.contains_key("db"));
        assert!(def.install.is_some());

        let install = def.install.as_ref().unwrap();
        assert!(install.requirements.contains_key("admin_user_email"));
        assert!(install.requirements.contains_key("admin_user_password"));
        drop(def);

        exercise_actions(&engine, &mut scope, &app, &ast);
    }
}
