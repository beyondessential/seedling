use super::*;
use defs::resource::ResourceKind;

// l[verify container.interface]
#[test]
fn container_is_an_interface_on_deployment_and_job() {
    run_test_script_app(
        r#"
        app.deployment("web")
            .image("nginx")
            .command("nginx")
            .arg("-g")
            .env("PORT", "80");

        app.job("task")
            .image("tools")
            .command("run")
            .arg("--fast")
            .env("MODE", "batch");
    "#,
    );
}

// l[verify container.image]
#[test]
fn container_image_sets_uri() {
    let app = run_test_script_app(
        r#"
        app.deployment("web").image("nginx:latest");
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
        assert_eq!(container.image.as_deref(), Some("nginx:latest"));
    } else {
        panic!("expected Deployment");
    }
}

// l[verify container.command]
#[test]
fn container_command_string() {
    let app = run_test_script_app(
        r#"
        app.job("task").command("run");
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
        assert_eq!(container.command.as_deref(), Some(&["run".to_string()][..]));
    } else {
        panic!("expected Job");
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

// l[verify container.arg]
#[test]
fn container_arg_single() {
    let app = run_test_script_app(
        r#"
        app.job("task").command("run").arg("--verbose");
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
            container.args.as_deref(),
            Some(&["--verbose".to_string()][..])
        );
    } else {
        panic!("expected Job");
    }
}

// l[verify container.arg]
#[test]
fn container_arg_array() {
    let app = run_test_script_app(
        r#"
        app.job("task").command("run").arg(["--verbose", "--dry-run"]);
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
            container.args.as_deref(),
            Some(&["--verbose".to_string(), "--dry-run".into()][..])
        );
    } else {
        panic!("expected Job");
    }
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

// l[verify container.mount-volume]
#[test]
fn container_mount_volume() {
    run_test_script_app(
        r#"
        let vol = app.volume("data");
        app.deployment("web")
            .image("nginx")
            .mount("/data", vol);
    "#,
    );
}

// l[verify container.mount-volume]
#[test]
fn container_mount_external_volume() {
    run_test_script_app(
        r#"
        let evol = app.external_volume("shared");
        app.deployment("web")
            .image("nginx")
            .mount("/shared", evol);
    "#,
    );
}

// l[verify container.on-exit]
#[test]
fn container_on_exit_strategy() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("nginx")
            .on_exit(OnExit.Terminate);
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
        assert!(matches!(container.on_exit, defs::enums::OnExit::Terminate));
    } else {
        panic!("expected Deployment");
    }
}
