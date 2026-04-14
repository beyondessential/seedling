use super::*;
use defs::resource::ResourceKind;

// l[verify container.interface]
#[test]
fn container_is_an_interface_on_deployment_and_job() {
    run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .command("nginx")
            .arg("-g")
            .env("PORT", "80");

        app.job("task")
            .image("docker.io/library/tools:latest")
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
        app.deployment("web").image("docker.io/library/nginx:latest");
    "#,
    );
    let def = app.def.lock();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        let pod = dep_def.pod.lock();
        let container = pod.container.lock();
        assert_eq!(
            container.image.as_deref(),
            Some("docker.io/library/nginx:latest")
        );
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
    let def = app.def.lock();
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
    let def = app.def.lock();
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
    let def = app.def.lock();
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
    let def = app.def.lock();
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
            .image("docker.io/library/app:1")
            .env("KEY", "old")
            .env("KEY", "new");
    "#,
    );
    let def = app.def.lock();
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
            .image("docker.io/library/nginx:latest")
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
            .image("docker.io/library/nginx:latest")
            .mount("/shared", evol);
    "#,
    );
}

// l[verify container.mount-volume.validation]
#[test]
fn mount_rejects_root() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/", vol);
    "#,
    );
}

// l[verify container.mount-volume.validation]
#[test]
fn mount_rejects_proc() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/proc", vol);
    "#,
    );
}

// l[verify container.mount-volume.validation]
#[test]
fn mount_rejects_sys() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/sys", vol);
    "#,
    );
}

// l[verify container.mount-volume.validation]
#[test]
fn mount_rejects_dev() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/dev", vol);
    "#,
    );
}

// l[verify container.mount-volume.validation]
#[test]
fn mount_rejects_etc() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/etc", vol);
    "#,
    );
}

// l[verify container.mount-volume.validation]
#[test]
fn mount_rejects_bin() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/bin", vol);
    "#,
    );
}

// l[verify container.mount-volume.validation]
#[test]
fn mount_rejects_usr() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/usr", vol);
    "#,
    );
}

// l[verify container.mount-volume.validation]
#[test]
fn mount_rejects_traversal_to_forbidden() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/data/../proc", vol);
    "#,
    );
}

// l[verify container.mount-volume.validation]
#[test]
fn mount_rejects_traversal_to_root() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/data/..", vol);
    "#,
    );
}

// l[verify container.mount-volume.validation]
#[test]
fn mount_rejects_repeated_slashes() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("///etc", vol);
    "#,
    );
}

// l[verify container.mount-volume.validation]
#[test]
fn mount_rejects_relative_path() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("data", vol);
    "#,
    );
}

// l[verify container.mount-volume.validation]
#[test]
fn mount_rejects_null_bytes() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/data\0evil", vol);
    "#,
    );
}

// l[verify container.mount-volume.validation]
#[test]
fn mount_accepts_valid_paths() {
    run_test_script_app(
        r#"
        let vol = app.volume("data");
        app.deployment("web")
            .mount("/data", vol)
            .mount("/var/lib/app", vol)
            .mount("/opt/myapp/storage", vol)
            .mount("/home/app", vol);
    "#,
    );
}

// l[verify container.mount-volume.validation]
#[test]
fn mount_accepts_subpath_of_forbidden() {
    run_test_script_app(
        r#"
        let vol = app.volume("data");
        app.deployment("web")
            .mount("/etc/myapp", vol)
            .mount("/proc-data", vol)
            .mount("/usr/local/share", vol);
    "#,
    );
}

// l[verify container.mount-volume.validation]
#[test]
fn mount_rejects_forbidden_external_volume() {
    let _ = run_test_script_err(
        r#"
        let evol = app.external_volume("shared");
        app.deployment("web").mount("/dev", evol);
    "#,
    );
}

// l[verify container.env.validation]
#[test]
fn env_rejects_path() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("PATH", "/usr/bin");
    "#,
    );
}

// l[verify container.env.validation]
#[test]
fn env_rejects_ld_preload() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("LD_PRELOAD", "/tmp/evil.so");
    "#,
    );
}

// l[verify container.env.validation]
#[test]
fn env_rejects_ld_library_path() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("LD_LIBRARY_PATH", "/tmp");
    "#,
    );
}

// l[verify container.env.validation]
#[test]
fn env_rejects_ld_audit() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("LD_AUDIT", "evil.so");
    "#,
    );
}

// l[verify container.env.validation]
#[test]
fn env_rejects_empty_name() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("", "value");
    "#,
    );
}

// l[verify container.env.validation]
#[test]
fn env_rejects_name_starting_with_digit() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("1BAD", "value");
    "#,
    );
}

// l[verify container.env.validation]
#[test]
fn env_rejects_name_with_special_chars() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("MY-VAR", "value");
    "#,
    );
}

// l[verify container.env.validation]
#[test]
fn env_rejects_null_in_value() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("GOOD_NAME", "bad\0value");
    "#,
    );
}

// l[verify container.env.validation]
#[test]
fn env_accepts_valid_names() {
    run_test_script_app(
        r#"
        app.deployment("web")
            .env("MY_VAR", "hello")
            .env("A", "short")
            .env("LONG_VARIABLE_NAME_123", "works");
    "#,
    );
}

// l[verify container.env.validation]
#[test]
fn env_map_form_rejects_forbidden() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env([#{ name: "LD_PRELOAD", value: "/tmp/evil.so" }]);
    "#,
    );
}

// l[verify container.on-exit]
#[test]
fn container_on_exit_strategy() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .on_exit(OnExit.Terminate);
    "#,
    );
    let def = app.def.lock();
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
