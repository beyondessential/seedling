use super::*;

// l[verify const.default-deadline]
#[test]
fn default_deadline_is_available() {
    run_test_script_app(
        r#"
        let d = DEFAULT_DEADLINE;
        if d <= 0 { throw "DEFAULT_DEADLINE must be positive non-zero"; }
    "#,
    );
}

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
