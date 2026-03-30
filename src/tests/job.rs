use super::*;
use defs::resource::ResourceKind;

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

// l[verify job.pod]
#[test]
fn job_implements_pod_interface() {
    let app = run_test_script_app(
        r#"
        let svc = app.service("ctrl");
        app.job("setup")
            .image("tools:1")
            .command("setup")
            .arg("--verbose")
            .env("MODE", "init")
            .tcp(9090, svc);
    "#,
    );
    let def = app.0.lock();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Job && &*id.name == "setup")
        .unwrap();
    if let defs::resource::Resource::Job(job) = &def.resources[id] {
        let job_def = job.def.lock();
        let pod = job_def.pod.lock();
        assert_eq!(pod.tcp_bindings.len(), 1);
        let container = pod.container.lock();
        assert_eq!(container.image.as_deref(), Some("tools:1"));
        assert_eq!(
            container.command.as_deref(),
            Some(&["setup".to_string()][..])
        );
        assert_eq!(
            container.args.as_deref(),
            Some(&["--verbose".to_string()][..])
        );
        assert_eq!(container.env.len(), 1);
    } else {
        panic!("expected Job");
    }
}

// l[verify job.deadline]
#[test]
fn job_deadline_rejects_zero() {
    let _ = run_test_script_err(
        r#"
        app.job("bad").deadline(0);
    "#,
    );
}

// l[verify job.deadline]
#[test]
fn job_deadline_rejects_negative() {
    let _ = run_test_script_err(
        r#"
        app.job("bad").deadline(-10);
    "#,
    );
}
