use std::path::PathBuf;

use rhai::{Engine, Scope};

mod defs;

fn setup() -> (Engine, Scope<'static>, defs::app::App) {
    let mut engine = Engine::new();
    defs::register(&mut engine);
    let (scope, app) = defs::scope();
    (engine, scope, app)
}

fn main() {
    let filepath = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("Usage: beset <RHAI FILE>"),
    );

    let (engine, mut scope, app) = setup();

    if let Err(err) = engine.run_file_with_scope(&mut scope, filepath) {
        eprintln!("{err}");
        std::process::exit(1);
    }

    let def = app.0.lock();
    println!("params: {:?}", def.params.keys().collect::<Vec<_>>());
    println!("resources: {}", def.resources.len());
    for id in def.resources.keys() {
        println!("  {:?} {:?}", id.kind, id.name);
    }
    println!("actions: {:?}", def.actions.keys().collect::<Vec<_>>());
    println!("shells: {:?}", def.shells.keys().collect::<Vec<_>>());
    println!("install: {}", def.install.is_some());
}

#[cfg(test)]
mod tests {
    use super::*;
    use defs::resource::ResourceKind;

    fn run_script(source: &str) -> defs::app::App {
        let (engine, mut scope, app) = setup();
        engine
            .run_with_scope(&mut scope, source)
            .expect("script should run without error");
        app
    }

    // l[verify param.type]
    // l[verify bsl.placeholder]
    #[test]
    fn param_returns_placeholder() {
        let app = run_script(r#"let x = app.param("foo");"#);
        let def = app.0.lock();
        assert!(def.params.contains_key("foo"));
        assert_eq!(def.params["foo"], "<placeholder>");
    }

    // l[verify service.type]
    // l[verify app.resources]
    #[test]
    fn service_creates_resource() {
        let app = run_script(r#"let s = app.service("web");"#);
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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
        let app = run_script(
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

    // l[verify rt.start]
    // l[verify rt.started.state-methods]
    #[test]
    fn runtime_instance_chain() {
        run_script(
            r#"
            app.on_start(|rt| {
                let svc = app.service("web");
                let dep = app.deployment("web").image("nginx");
                rt.start(svc);
                rt.start(dep).ready();
            });
        "#,
        );
    }

    // l[verify rt.started.terminated]
    // l[verify rt.termination.ensure-success]
    #[test]
    fn started_terminated_ensure_success() {
        run_script(
            r#"
            app.on_start(|rt| {
                let job = app.job("init").image("tools").command("setup");
                rt.start(job).terminated().ensure_success();
            });
        "#,
        );
    }

    // l[verify const.available-threads]
    #[test]
    fn available_threads_constant() {
        run_script(
            r#"
            app.deployment("workers")
                .scale(1..AVAILABLE_THREADS);
        "#,
        );
    }

    // l[verify app.resources.dynamic]
    #[test]
    fn closures_create_dynamic_resources() {
        let app = run_script(
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

    // l[verify bsl.script]
    #[test]
    fn draft_script_runs() {
        let (engine, mut scope, app) = setup();
        engine
            .run_file_with_scope(&mut scope, std::path::PathBuf::from("draft-beset.rhai"))
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
    }
}
