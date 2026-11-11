use super::*;
use defs::resource::ResourceKind;

// spec: CTR#container-interface
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

// spec: CTR#image
#[test]
fn container_image_sets_uri() {
    let app = run_test_script_app(
        r#"
        app.deployment("web").image("docker.io/library/nginx:latest");
    "#,
    );
    let def = app.def.load();
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

// spec: CTR#image
#[test]
fn image_rejects_bare_name() {
    let _ = run_test_script_err(r#"app.deployment("web").image("nginx");"#);
}

// spec: CTR#image
#[test]
fn image_rejects_name_with_tag_but_no_registry() {
    let _ = run_test_script_err(r#"app.deployment("web").image("nginx:latest");"#);
}

// spec: CTR#image
#[test]
fn image_rejects_no_tag_or_digest() {
    let _ = run_test_script_err(r#"app.deployment("web").image("docker.io/library/nginx");"#);
}

// spec: CTR#image
#[test]
fn image_accepts_registry_with_tag() {
    let app =
        run_test_script_app(r#"app.deployment("web").image("docker.io/library/nginx:latest");"#);
    let def = app.def.load();
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

// spec: CTR#image
#[test]
fn image_accepts_digest() {
    run_test_script_app(
        r#"app.deployment("web").image("docker.io/library/nginx@sha256:abcdef0123456789abcdef0123456789");"#,
    );
}

// spec: CTR#image
#[test]
fn image_accepts_registry_with_port() {
    run_test_script_app(
        r#"app.deployment("web").image("myregistry.io:5000/myapp/server:v1.2.3");"#,
    );
}

// spec: CTR#image
#[test]
fn image_accepts_ghcr() {
    run_test_script_app(r#"app.deployment("web").image("ghcr.io/owner/repo:main");"#);
}

// spec: CTR#image
#[test]
fn image_rejects_empty() {
    let _ = run_test_script_err(r#"app.deployment("web").image("");"#);
}

// spec: CTR#image
#[test]
fn image_rejects_invalid_digest() {
    let _ = run_test_script_err(
        r#"app.deployment("web").image("docker.io/library/nginx@sha256:short");"#,
    );
}

// spec: CTR#command-and-arguments
#[test]
fn container_command_string() {
    let app = run_test_script_app(
        r#"
        app.job("task").command("run");
    "#,
    );
    let def = app.def.load();
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

// spec: CTR#command-and-arguments
#[test]
fn container_command_array() {
    let app = run_test_script_app(
        r#"
        app.job("task")
            .command(["bash", "-c", "echo hello"]);
    "#,
    );
    let def = app.def.load();
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

// spec: CTR#command-and-arguments
#[test]
fn container_arg_single() {
    let app = run_test_script_app(
        r#"
        app.job("task").command("run").arg("--verbose");
    "#,
    );
    let def = app.def.load();
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

// spec: CTR#command-and-arguments
#[test]
fn container_arg_array() {
    let app = run_test_script_app(
        r#"
        app.job("task").command("run").arg(["--verbose", "--dry-run"]);
    "#,
    );
    let def = app.def.load();
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

// spec: CTR#environment
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
    let def = app.def.load();
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
        assert_eq!(container.env[0].name.as_str(), "KEY");
        assert_eq!(container.env[0].value, "new");
    } else {
        panic!("expected Deployment");
    }
}

// spec: CTR#volume-mounts
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

// spec: CTR#volume-mounts
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

// spec: CTR#volume-mounts
#[test]
fn mount_rejects_root() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/", vol);
    "#,
    );
}

// spec: CTR#volume-mounts
#[test]
fn mount_rejects_proc() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/proc", vol);
    "#,
    );
}

// spec: CTR#volume-mounts
#[test]
fn mount_rejects_sys() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/sys", vol);
    "#,
    );
}

// spec: CTR#volume-mounts
#[test]
fn mount_rejects_dev() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/dev", vol);
    "#,
    );
}

// spec: CTR#volume-mounts
#[test]
fn mount_rejects_etc() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/etc", vol);
    "#,
    );
}

// spec: CTR#volume-mounts
#[test]
fn mount_rejects_bin() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/bin", vol);
    "#,
    );
}

// spec: CTR#volume-mounts
#[test]
fn mount_rejects_usr() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/usr", vol);
    "#,
    );
}

// spec: CTR#volume-mounts
#[test]
fn mount_rejects_traversal_to_forbidden() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/data/../proc", vol);
    "#,
    );
}

// spec: CTR#volume-mounts
#[test]
fn mount_rejects_traversal_to_root() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/data/..", vol);
    "#,
    );
}

// spec: CTR#volume-mounts
#[test]
fn mount_rejects_repeated_slashes() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("///etc", vol);
    "#,
    );
}

// spec: CTR#volume-mounts
#[test]
fn mount_rejects_relative_path() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("data", vol);
    "#,
    );
}

// spec: CTR#volume-mounts
#[test]
fn mount_rejects_null_bytes() {
    let _ = run_test_script_err(
        r#"
        let vol = app.volume("data");
        app.deployment("web").mount("/data\0evil", vol);
    "#,
    );
}

// spec: CTR#volume-mounts
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

// spec: CTR#volume-mounts
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

// spec: CTR#volume-mounts
#[test]
fn mount_rejects_forbidden_external_volume() {
    let _ = run_test_script_err(
        r#"
        let evol = app.external_volume("shared");
        app.deployment("web").mount("/dev", evol);
    "#,
    );
}

// spec: CTR#environment
#[test]
fn env_rejects_path() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("PATH", "/usr/bin");
    "#,
    );
}

// spec: CTR#environment
#[test]
fn env_rejects_ld_preload() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("LD_PRELOAD", "/tmp/evil.so");
    "#,
    );
}

// spec: CTR#environment
#[test]
fn env_rejects_ld_library_path() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("LD_LIBRARY_PATH", "/tmp");
    "#,
    );
}

// spec: CTR#environment
#[test]
fn env_rejects_ld_audit() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("LD_AUDIT", "evil.so");
    "#,
    );
}

// spec: CTR#environment
#[test]
fn env_rejects_empty_name() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("", "value");
    "#,
    );
}

// spec: CTR#environment
#[test]
fn env_rejects_name_starting_with_digit() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("1BAD", "value");
    "#,
    );
}

// spec: CTR#environment
#[test]
fn env_rejects_name_with_special_chars() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("MY-VAR", "value");
    "#,
    );
}

// spec: CTR#environment
#[test]
fn env_rejects_null_in_value() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env("GOOD_NAME", "bad\0value");
    "#,
    );
}

// spec: CTR#environment
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

// spec: CTR#environment
#[test]
fn env_map_form_rejects_forbidden() {
    let _ = run_test_script_err(
        r#"
        app.deployment("web").env([#{ name: "LD_PRELOAD", value: "/tmp/evil.so" }]);
    "#,
    );
}

// spec: CTR#working-directory
#[test]
fn container_workdir_sets_path() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .workdir("/app");
    "#,
    );
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
        .unwrap();
    if let defs::resource::Resource::Deployment(dep) = &def.resources[id] {
        let dep_def = dep.def.lock();
        let pod = dep_def.pod.lock();
        let container = pod.container.lock();
        assert_eq!(container.workdir.as_deref(), Some("/app"));
    } else {
        panic!("expected Deployment");
    }
}

// spec: CTR#working-directory
#[test]
fn container_workdir_rejects_relative_path() {
    let _ = run_test_script_err(r#"app.deployment("web").workdir("app/relative");"#);
}

// spec: CTR#exit-strategy
#[test]
fn container_on_exit_strategy() {
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
        .find(|id| id.kind == ResourceKind::Deployment && &*id.name == "web")
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

fn with_container<R>(
    app: &crate::defs::app::App,
    name: &str,
    f: impl FnOnce(&defs::container::ContainerDef) -> R,
) -> R {
    let def = app.def.load();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Deployment && *id.name == *name)
        .expect("deployment not found");
    let defs::resource::Resource::Deployment(dep) = &def.resources[id] else {
        panic!("expected Deployment");
    };
    let dep_def = dep.def.lock();
    let pod = dep_def.pod.lock();
    let container = pod.container.lock();
    f(&container)
}

// spec: CTR#resource-limits
#[test]
fn container_memory_sets_limit() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .memory("512m");
    "#,
    );
    with_container(&app, "web", |c| {
        assert_eq!(c.memory.as_deref(), Some("512m"));
    });
}

// spec: CTR#resource-limits
#[test]
fn container_memory_case_insensitive_suffix() {
    let app = run_test_script_app(
        r#"app.deployment("web").image("docker.io/library/nginx:latest").memory("2G");"#,
    );
    with_container(&app, "web", |c| {
        assert_eq!(c.memory.as_deref(), Some("2g"));
    });
}

// spec: CTR#resource-limits
#[test]
fn container_memory_rejects_missing_suffix() {
    let _ = run_test_script_err(
        r#"app.deployment("web").image("docker.io/library/nginx:latest").memory("1024");"#,
    );
}

// spec: CTR#resource-limits
#[test]
fn container_memory_rejects_unknown_suffix() {
    let _ = run_test_script_err(
        r#"app.deployment("web").image("docker.io/library/nginx:latest").memory("1t");"#,
    );
}

// spec: CTR#resource-limits
#[test]
fn container_cpus_sets_limit() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .cpus(1.5);
    "#,
    );
    with_container(&app, "web", |c| {
        assert_eq!(c.cpus, Some(1.5));
    });
}

// spec: CTR#resource-limits
#[test]
fn container_cpus_rejects_zero() {
    let _ = run_test_script_err(
        r#"app.deployment("web").image("docker.io/library/nginx:latest").cpus(0);"#,
    );
}

// spec: CTR#resource-limits
#[test]
fn container_cpus_rejects_negative() {
    let _ = run_test_script_err(
        r#"app.deployment("web").image("docker.io/library/nginx:latest").cpus(-0.5);"#,
    );
}

// spec: CTR#privileges
#[test]
fn container_cap_add_appends_capability() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .cap_add("NET_BIND_SERVICE");
    "#,
    );
    with_container(&app, "web", |c| {
        assert_eq!(c.extra_caps, vec!["NET_BIND_SERVICE".to_owned()]);
    });
}

// spec: CTR#privileges
#[test]
fn container_cap_add_normalises_to_uppercase() {
    let app = run_test_script_app(
        r#"app.deployment("web").image("docker.io/library/nginx:latest").cap_add("net_raw");"#,
    );
    with_container(&app, "web", |c| {
        assert_eq!(c.extra_caps, vec!["NET_RAW".to_owned()]);
    });
}

// spec: CTR#privileges
#[test]
fn container_cap_add_deduplicates() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .cap_add("NET_RAW")
            .cap_add("net_raw");
    "#,
    );
    with_container(&app, "web", |c| {
        assert_eq!(c.extra_caps, vec!["NET_RAW".to_owned()]);
    });
}

// spec: CTR#privileges
#[test]
fn container_writable_rootfs_defaults_false() {
    let app =
        run_test_script_app(r#"app.deployment("web").image("docker.io/library/nginx:latest");"#);
    with_container(&app, "web", |c| {
        assert!(!c.writable_rootfs);
    });
}

// spec: CTR#privileges
#[test]
fn container_writable_rootfs_opts_in() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .writable_rootfs();
    "#,
    );
    with_container(&app, "web", |c| {
        assert!(c.writable_rootfs);
    });
}

// spec: CTR#resource-limits
#[test]
fn container_pids_limit_sets_value() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .pids_limit(512);
    "#,
    );
    with_container(&app, "web", |c| {
        assert_eq!(c.pids_limit, Some(512));
    });
}

// spec: CTR#resource-limits
#[test]
fn container_pids_limit_rejects_zero() {
    let _ = run_test_script_err(
        r#"app.deployment("web").image("docker.io/library/nginx:latest").pids_limit(0);"#,
    );
}

// spec: CTR#resource-limits
#[test]
fn container_pids_limit_rejects_negative() {
    let _ = run_test_script_err(
        r#"app.deployment("web").image("docker.io/library/nginx:latest").pids_limit(-1);"#,
    );
}

// spec: CTR#stopping
#[test]
fn container_stop_signal_canonicalises_signal_name() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .stop_signal("SIGINT");
        app.deployment("api")
            .image("docker.io/library/nginx:latest")
            .stop_signal("int");
    "#,
    );
    with_container(&app, "web", |c| {
        assert_eq!(c.stop_signal.as_deref(), Some("SIGINT"));
    });
    with_container(&app, "api", |c| {
        assert_eq!(c.stop_signal.as_deref(), Some("SIGINT"));
    });
}

// spec: CTR#stopping
#[test]
fn container_stop_signal_rejects_unknown() {
    let _ = run_test_script_err(
        r#"app.deployment("web").image("docker.io/library/nginx:latest").stop_signal("DESTRUCT");"#,
    );
}

// spec: CTR#stopping
#[test]
fn container_stop_timeout_sets_value() {
    let app = run_test_script_app(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:latest")
            .stop_timeout(60);
    "#,
    );
    with_container(&app, "web", |c| {
        assert_eq!(c.stop_timeout_secs, Some(60));
    });
}

// spec: CTR#stopping
#[test]
fn container_stop_timeout_rejects_zero() {
    let _ = run_test_script_err(
        r#"app.deployment("web").image("docker.io/library/nginx:latest").stop_timeout(0);"#,
    );
}

// spec: BSL#arguments
// spec: CTR#resource-limits
// The unchecked `as u32` wrapped: pids_limit(4294967297) became 1, and the
// container could not start a workload.
#[test]
fn pids_limit_rejects_values_beyond_u32() {
    let err = run_test_script_err(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:1.29")
            .pids_limit(4294967297);
    "#,
    );
    let message = err.to_string();
    assert!(message.contains("`pids_limit`"), "{message}");
}

// spec: BSL#arguments
// spec: CTR#stopping
#[test]
fn stop_timeout_rejects_values_beyond_u32() {
    let err = run_test_script_err(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:1.29")
            .stop_timeout(4294967297);
    "#,
    );
    assert!(err.to_string().contains("`stop_timeout`"), "{err}");
}

// spec: BSL#arguments
// spec: CTR#command-and-arguments
#[test]
fn command_array_rejects_non_string_elements() {
    let err = run_test_script_err(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:1.29")
            .command(["nginx", 8080]);
    "#,
    );
    let message = err.to_string();
    assert!(message.contains("`command`"), "{message}");
    assert!(message.contains("element 1"), "{message}");
}

// spec: BSL#arguments
// spec: CTR#command-and-arguments
#[test]
fn arg_array_rejects_non_string_elements() {
    let err = run_test_script_err(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:1.29")
            .arg(["-c", 3]);
    "#,
    );
    assert!(err.to_string().contains("element 1"), "{err}");
}

// spec: BSL#arguments
// spec: CTR#environment
// A non-map entry used to be skipped, so the variable simply did not exist.
#[test]
fn env_array_rejects_non_map_entries() {
    let err = run_test_script_err(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx:1.29")
            .env(["DB_HOST=localhost"]);
    "#,
    );
    let message = err.to_string();
    assert!(message.contains("env entry 0"), "{message}");
    assert!(message.contains("must be a map"), "{message}");
}

// spec: BSL#arguments
// spec: CTR#image
// `is_ascii_lowercase` accepts g–z, so a digest of g's passed as hex.
#[test]
fn image_digest_must_be_hex() {
    let err = run_test_script_err(
        r#"
        app.deployment("web")
            .image("docker.io/library/nginx@sha256:gggggggggggggggggggggggggggggggg");
    "#,
    );
    assert!(err.to_string().contains("digest hex"), "{err}");
}
