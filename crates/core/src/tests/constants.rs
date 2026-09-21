use super::*;

use crate::defs::{enums::Priority, resource::Resource};

// l[verify const.available-threads]
#[test]
fn available_threads_is_positive() {
    run_test_script_app(
        r#"
        if AVAILABLE_THREADS <= 0 { throw "AVAILABLE_THREADS must be positive non-zero"; }
    "#,
    );
}

// l[verify const.available-threads]
#[test]
fn available_threads_usable_in_scale() {
    run_test_script_app(
        r#"
        app.deployment("workers")
            .scale(1..AVAILABLE_THREADS);
    "#,
    );
}

// l[verify const.available-memory]
#[test]
fn available_memory_is_positive() {
    run_test_script_app(
        r#"
        if AVAILABLE_MEMORY <= 0 { throw "AVAILABLE_MEMORY must be positive non-zero"; }
    "#,
    );
}

// l[verify const.cpu-architecture]
#[test]
fn cpu_architecture_is_nonempty_string() {
    run_test_script_app(
        r#"
        if type_of(CPU_ARCHITECTURE) != "string" {
            throw "CPU_ARCHITECTURE must be a string";
        }
        if CPU_ARCHITECTURE == "" {
            throw "CPU_ARCHITECTURE must not be empty";
        }
    "#,
    );
}

// l[verify const.host-has-ipv4]
#[test]
fn host_has_ipv4_is_bool() {
    run_test_script_app(
        r#"
        if type_of(HOST_HAS_IPV4) != "bool" {
            throw "HOST_HAS_IPV4 must be a bool";
        }
    "#,
    );
}

// l[verify const.host-has-ipv6]
#[test]
fn host_has_ipv6_is_bool() {
    run_test_script_app(
        r#"
        if type_of(HOST_HAS_IPV6) != "bool" {
            throw "HOST_HAS_IPV6 must be a bool";
        }
    "#,
    );
}

// l[verify const.nat64-active]
#[test]
fn nat64_active_is_bool() {
    run_test_script_app(
        r#"
        if type_of(NAT64_ACTIVE) != "bool" {
            throw "NAT64_ACTIVE must be a bool";
        }
    "#,
    );
}

// l[verify const.has-snapshots]
#[test]
fn has_snapshots_is_bool() {
    run_test_script_app(
        r#"
        if type_of(HAS_SNAPSHOTS) != "bool" {
            throw "HAS_SNAPSHOTS must be a bool";
        }
    "#,
    );
}

// l[verify const.node-name]
#[test]
fn node_name_is_string() {
    run_test_script_app(
        r#"
        if type_of(NODE_NAME) != "string" {
            throw "NODE_NAME must be a string";
        }
    "#,
    );
}

// l[verify const.timezone]
#[test]
fn timezone_is_nonempty_string() {
    run_test_script_app(
        r#"
        if type_of(TIMEZONE) != "string" {
            throw "TIMEZONE must be a string";
        }
        if TIMEZONE == "" {
            throw "TIMEZONE must not be empty";
        }
    "#,
    );
}

// l[verify const.on-update.rolling]
#[test]
fn on_update_rolling() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .on_update(OnUpdate.Rolling);
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == defs::resource::ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        assert!(matches!(dep_def.on_update, defs::enums::OnUpdate::Rolling));
    } else {
        panic!("expected Deployment");
    }
}

// l[verify const.on-update.replace]
#[test]
fn on_update_replace() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .on_update(OnUpdate.Replace);
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == defs::resource::ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        assert!(matches!(dep_def.on_update, defs::enums::OnUpdate::Replace));
    } else {
        panic!("expected Deployment");
    }
}

// l[verify const.on-terminate.recreate]
#[test]
fn on_terminate_recreate() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .on_terminate(OnTerminate.Recreate);
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == defs::resource::ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        assert!(matches!(
            dep_def.on_terminate,
            defs::enums::OnTerminate::Recreate
        ));
    } else {
        panic!("expected Deployment");
    }
}

// l[verify const.on-exit.restart]
#[test]
fn on_exit_restart() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .on_exit(OnExit.Restart);
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == defs::resource::ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        let pod = dep_def.pod.lock();
        let container = pod.container.lock();
        assert!(matches!(
            container.on_exit,
            Some(defs::enums::OnExit::Restart)
        ));
    } else {
        panic!("expected Deployment");
    }
}

// l[verify const.on-exit.terminate]
#[test]
fn on_exit_terminate() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .on_exit(OnExit.Terminate);
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == defs::resource::ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        let pod = dep_def.pod.lock();
        let container = pod.container.lock();
        assert!(matches!(
            container.on_exit,
            Some(defs::enums::OnExit::Terminate)
        ));
    } else {
        panic!("expected Deployment");
    }
}

// l[verify const.on-exit.restart-on-failure]
#[test]
fn on_exit_restart_on_failure() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .on_exit(OnExit.RestartOnFailure);
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == defs::resource::ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        let pod = dep_def.pod.lock();
        let container = pod.container.lock();
        assert!(matches!(
            container.on_exit,
            Some(defs::enums::OnExit::RestartOnFailure)
        ));
    } else {
        panic!("expected Deployment");
    }
}

// l[verify const.resource-type.enum]
#[test]
fn resource_type_enum_variants_accessible() {
    run_test_script_app(
        r#"
        let _p = ResourceType.Parameter;
        let _s = ResourceType.Service;
        let _h = ResourceType.HttpService;
        let _i = ResourceType.Ingress;
        let _d = ResourceType.Deployment;
        let _j = ResourceType.Job;
        let _v = ResourceType.Volume;
        let _ev = ResourceType.ExternalVolume;
        let _a = ResourceType.Action;
    "#,
    );
}

// l[verify const.priority.enum]
#[test]
fn priority_exposes_the_four_levels() {
    run_test_script_app(
        r#"
        for level in ["Critical", "Elevated", "Normal", "Low"] {
            if !Priority.contains(level) {
                throw "Priority is missing " + level;
            }
        }
    "#,
    );
}

// l[verify deployment.priority]
#[test]
fn deployment_accepts_every_priority_level() {
    let app = run_test_script_app(
        r#"
        app.deployment("api").image("docker.io/library/nginx:1").priority(Priority.Critical);
        app.deployment("sync").image("docker.io/library/nginx:1").priority(Priority.Elevated);
        app.deployment("web").image("docker.io/library/nginx:1").priority(Priority.Normal);
        app.deployment("batch").image("docker.io/library/nginx:1").priority(Priority.Low);
    "#,
    );
    let def = app.def.load();
    let levels: std::collections::BTreeMap<String, Priority> = def
        .resources
        .iter()
        .filter_map(|(id, r)| match r {
            Resource::Deployment(d) => Some((id.name.as_str().to_owned(), d.def.lock().priority)),
            _ => None,
        })
        .collect();
    assert_eq!(levels["api"], Priority::Critical);
    assert_eq!(levels["sync"], Priority::Elevated);
    assert_eq!(levels["web"], Priority::Normal);
    assert_eq!(levels["batch"], Priority::Low);
}

// l[verify deployment.priority]
#[test]
fn a_deployment_that_declares_no_priority_is_normal() {
    let app = run_test_script_app(r#"app.deployment("web").image("docker.io/library/nginx:1");"#);
    let def = app.def.load();
    let (_, resource) = def
        .resources
        .iter()
        .find(|(id, _)| id.name.as_str() == "web")
        .expect("deployment should exist");
    let Resource::Deployment(d) = resource else {
        panic!("expected a deployment");
    };
    assert_eq!(d.def.lock().priority, Priority::Normal);
}

// l[verify deployment.priority]
// Priority is declared on Deployments only, so the method does not exist on a
// Job and calling it is an evaluation error rather than a silent no-op.
#[test]
fn priority_is_not_available_on_a_job() {
    let err = run_test_script_err(
        r#"app.job("migrate").image("docker.io/library/nginx:1").priority(Priority.Critical);"#,
    );
    let msg = err.to_string();
    assert!(
        msg.contains("priority") && msg.contains("Job"),
        "error should name the unavailable method and the type it is missing from, got: {msg}"
    );
}

// l[verify deployment.priority]
#[test]
fn priority_is_chainable_with_other_builders() {
    let app = run_test_script_app(
        r#"
        app.deployment("api")
            .image("docker.io/library/nginx:1")
            .priority(Priority.Critical)
            .scale(2);
    "#,
    );
    let def = app.def.load();
    let (_, resource) = def
        .resources
        .iter()
        .find(|(id, _)| id.name.as_str() == "api")
        .expect("deployment should exist");
    let Resource::Deployment(d) = resource else {
        panic!("expected a deployment");
    };
    let d = d.def.lock();
    assert_eq!(d.priority, Priority::Critical);
    assert_eq!(d.scale.start, 2);
}
