use std::sync::Arc;

use seedling_protocol::names::SessionId;

use super::{
    registry::StopSignal,
    session::{SessionOutcome, SessionProgress, run_app_shell, session_outcome},
};
use crate::{
    oi::test_support::TestOi,
    runtime::{
        apps::DynamicSlot,
        barrier::shell::{ShellExecTarget, ShellExit, ShellHost},
    },
};

#[cfg(unix)]
mod jobs;

/// A host standing in for the daemon's: it records each attachment it is asked
/// for and answers with a fixed result, after `hold` if given.
struct FakeHost {
    result: Result<ShellExit, String>,
    hold: Option<std::time::Duration>,
    attached: std::sync::atomic::AtomicUsize,
    finished: std::sync::atomic::AtomicBool,
    jobs: parking_lot::Mutex<Vec<String>>,
}

impl FakeHost {
    fn answering(result: Result<ShellExit, String>) -> Arc<Self> {
        Arc::new(Self {
            result,
            hold: None,
            attached: std::sync::atomic::AtomicUsize::new(0),
            finished: std::sync::atomic::AtomicBool::new(false),
            jobs: parking_lot::Mutex::new(Vec::new()),
        })
    }

    fn holding(result: Result<ShellExit, String>, hold: std::time::Duration) -> Arc<Self> {
        Arc::new(Self {
            hold: Some(hold),
            ..Arc::into_inner(Self::answering(result)).unwrap()
        })
    }

    fn attachments(&self) -> usize {
        self.attached.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl ShellHost for FakeHost {
    fn attach(&self, target: ShellExecTarget) -> Result<ShellExit, String> {
        self.attached
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.jobs.lock().push(target.job_name);
        if let Some(hold) = self.hold {
            std::thread::sleep(hold);
        }
        self.finished
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.result.clone()
    }
}

/// A session run to its end, with the app it ran on.
struct ShellRan {
    oi: TestOi,
    outcome: SessionOutcome,
    session_id: SessionId,
    /// The slot the session placed its dynamic resources in.
    slot: DynamicSlot,
}

/// Run the `tools` shell of a `demo` app declared by `script`, with `params`,
/// against `host`, stopping the session after `stop_after` if given, and say
/// how the session ended.
fn run_shell(
    script: &str,
    params: serde_json::Value,
    host: &Arc<FakeHost>,
    stop_after: Option<std::time::Duration>,
) -> SessionOutcome {
    run_shell_session(script, params, host, stop_after, |_| {}).outcome
}

/// As [`run_shell`], keeping the app and the session's slot for inspection,
/// and calling `setup` on the app before the session runs.
fn run_shell_session(
    script: &str,
    params: serde_json::Value,
    host: &Arc<FakeHost>,
    stop_after: Option<std::time::Duration>,
    setup: impl FnOnce(&TestOi),
) -> ShellRan {
    use seedling_protocol::names::ShellName;

    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        serde_json::json!({ "app": "demo", "script": script }),
    )
    .unwrap();
    setup(&oi);
    let (app, schema, sessions, tick_notify) = {
        let reg = oi.state.registry.read();
        let entry = reg.get("demo").unwrap();
        let schema = entry
            .app
            .def
            .load()
            .shells
            .get(&"tools".parse::<ShellName>().unwrap())
            .unwrap()
            .params
            .clone();
        (
            entry.app.clone(),
            schema,
            Arc::clone(&entry.sessions),
            Arc::clone(&entry.tick_notify),
        )
    };
    let params = crate::runtime::param_map::ParamMap::from_values(
        params.as_object().cloned().unwrap_or_default(),
    );
    let stop = StopSignal::new();
    let session_id = SessionId::generate();
    let slot = sessions.open(session_id);
    let outcome = oi.block_on(async {
        if let Some(after) = stop_after {
            let stop = stop.clone();
            tokio::spawn(async move {
                tokio::time::sleep(after).await;
                stop.request();
            });
        }
        let run = run_app_shell(
            &oi.state,
            app,
            &"demo".parse().unwrap(),
            &"tools".parse().unwrap(),
            &schema,
            params,
            &stop,
            Arc::clone(host) as Arc<dyn ShellHost>,
            SessionProgress {
                id: session_id,
                slot: Arc::clone(&slot),
                tick_notify,
            },
        )
        .await;
        let job_exit = host
            .result
            .as_ref()
            .ok()
            .and_then(|exit| exit.exit_code())
            .filter(|_| host.attachments() > 0);
        session_outcome(&run, stop.is_requested(), job_exit)
    });
    ShellRan {
        oi,
        outcome,
        session_id,
        slot,
    }
}

const TOOLS_JOB: &str = r#"
    let tools = app.job("tools").image("docker.io/library/busybox:1.37").command("sh");
"#;

/// Why the session of `body` as the `tools` shell's closure failed.
fn failure_reason(body: &str, host: &Arc<FakeHost>) -> Option<String> {
    let script = format!(r#"{TOOLS_JOB} app.on_shell("tools", |rt, shell, p| {{ {body} }});"#);
    run_shell(&script, serde_json::json!({}), host, None).reason
}

// spec: ACN#shell-control
#[test]
fn shell_error_delivers_its_message_to_the_client() {
    let reason = failure_reason(
        r#"shell.error("not today");"#,
        &FakeHost::answering(Ok(ShellExit::ended())),
    );
    assert_eq!(reason.as_deref(), Some("not today"));
}

// spec: ACN#shell-control
// spec: ACN#shellexit
#[test]
fn attach_returns_once_the_attachment_ends_and_the_closure_goes_on() {
    let host = FakeHost::holding(
        Ok(ShellExit::exited(3)),
        std::time::Duration::from_millis(300),
    );
    let reason = failure_reason(
        r#"
        let exit = shell.attach(tools);
        shell.error("exited=" + exit.exited() + " code=" + exit.exit_code() + " ok=" + exit.success());
        "#,
        &host,
    );
    assert_eq!(reason.as_deref(), Some("exited=true code=3 ok=false"));
    assert!(
        host.finished.load(std::sync::atomic::Ordering::SeqCst),
        "the closure went on only after the attachment ended"
    );
    assert_eq!(host.jobs.lock().as_slice(), ["tools"]);
}

// spec: ACN#shellexit
#[test]
fn an_attachment_that_ended_before_the_job_exited_has_no_exit_code() {
    let host = FakeHost::answering(Ok(ShellExit::ended()));
    let reason = failure_reason(
        r#"
        let exit = shell.attach(tools);
        shell.error("exited=" + exit.exited() + " unit=" + (exit.exit_code() == ()) + " ok=" + exit.success());
        "#,
        &host,
    );
    assert_eq!(reason.as_deref(), Some("exited=false unit=true ok=false"));
}

// spec: ACN#shellexit
#[test]
fn ensure_success_throws_unless_the_job_exited_with_zero() {
    let reason = failure_reason(
        r#"shell.attach(tools).ensure_success();"#,
        &FakeHost::answering(Ok(ShellExit::ended())),
    )
    .expect("the session failed");
    assert!(reason.contains("did not exit"), "{reason}");

    let reason = failure_reason(
        r#"shell.attach(tools).ensure_success();"#,
        &FakeHost::answering(Ok(ShellExit::exited(7))),
    )
    .expect("the session failed");
    assert!(reason.contains("exit code 7"), "{reason}");

    let host = FakeHost::answering(Ok(ShellExit::exited(0)));
    let script = format!(
        r#"{TOOLS_JOB} app.on_shell("tools", |rt, shell, p| {{ shell.attach(tools).ensure_success(); }});"#
    );
    let outcome = run_shell(&script, serde_json::json!({}), &host, None);
    assert_eq!((outcome.exit_code, outcome.reason), (0, None));
}

// spec: SHL#session-end
#[test]
fn a_session_whose_job_exited_ends_with_its_exit_code() {
    let host = FakeHost::answering(Ok(ShellExit::exited(5)));
    let script =
        format!(r#"{TOOLS_JOB} app.on_shell("tools", |rt, shell, p| {{ shell.attach(tools); }});"#);
    let outcome = run_shell(&script, serde_json::json!({}), &host, None);
    assert_eq!((outcome.exit_code, outcome.reason), (5, None));
}

// spec: ACN#shell-control
#[test]
fn a_second_attach_in_one_run_throws() {
    let host = FakeHost::answering(Ok(ShellExit::exited(0)));
    let reason = failure_reason(r#"shell.attach(tools); shell.attach(tools);"#, &host)
        .expect("the session failed");
    assert!(reason.contains("already called"), "{reason}");
    assert_eq!(host.attachments(), 1);
}

// spec: ACN#shell-control
#[test]
fn a_closure_that_returns_without_attaching_ends_the_session_in_error() {
    let host = FakeHost::answering(Ok(ShellExit::exited(0)));
    let script = format!(r#"{TOOLS_JOB} app.on_shell("tools", |rt, shell, p| {{ }});"#);
    let outcome = run_shell(&script, serde_json::json!({}), &host, None);
    assert_eq!(outcome.exit_code, -1);
    assert_eq!(outcome.reason.as_deref(), Some("the shell did not attach"));
}

// spec: SHL#session-startup
#[test]
fn a_failure_after_attach_ends_the_session_with_minus_one() {
    let host = FakeHost::answering(Ok(ShellExit::exited(0)));
    let script = format!(
        r#"{TOOLS_JOB} app.on_shell("tools", |rt, shell, p| {{ shell.attach(tools); throw "cleanup failed"; }});"#
    );
    let outcome = run_shell(&script, serde_json::json!({}), &host, None);
    assert_eq!(outcome.exit_code, -1);
    assert!(
        outcome.reason.is_some_and(|r| r.contains("cleanup failed")),
        "the reason is written to stderr"
    );
}

// spec: SHL#session-startup
#[test]
fn a_job_that_cannot_be_attached_fails_the_session_with_its_reason() {
    let host = FakeHost::answering(Err("exec failed: no such image".to_owned()));
    let script =
        format!(r#"{TOOLS_JOB} app.on_shell("tools", |rt, shell, p| {{ shell.attach(tools); }});"#);
    let outcome = run_shell(&script, serde_json::json!({}), &host, None);
    assert_eq!(outcome.exit_code, -1);
    assert_eq!(
        outcome.reason.as_deref(),
        Some("exec failed: no such image")
    );
}

// spec: SHL#session-end
#[test]
fn a_stop_after_the_attachment_ended_waits_for_the_script() {
    let host = FakeHost::answering(Ok(ShellExit::exited(0)));
    let script = format!(
        r#"{TOOLS_JOB} app.on_shell("tools", |rt, shell, p| {{
            shell.attach(tools);
            rt.start(app.job().image("docker.io/library/busybox:1.37").command("true")).terminated();
        }});"#
    );
    let began = std::time::Instant::now();
    let outcome = run_shell(
        &script,
        serde_json::json!({}),
        &host,
        Some(std::time::Duration::from_millis(2500)),
    );
    assert!(
        began.elapsed() >= std::time::Duration::from_millis(2500),
        "the session ran on after its attachment ended, until the stop"
    );
    assert_eq!(
        host.attachments(),
        1,
        "a rerun of the closure does not attach again"
    );
    assert_eq!(
        outcome.reason, None,
        "a stopped session is not a failed one"
    );
}

// spec: ACN#volume-params
#[test]
fn a_shell_whose_volume_param_cannot_be_bound_does_not_run_its_closure() {
    // The volume was validated and has since gone; the closure would have run
    // with the key missing.
    let reason = run_shell(
        r#"
        app.on_shell("tools", |rt, shell, p| { shell.error("the closure ran"); },
            #{ params: #{ data: #{ kind: "volume", required: true } } });
    "#,
        serde_json::json!({ "data": "gone-volume" }),
        &FakeHost::answering(Ok(ShellExit::ended())),
        None,
    )
    .reason;
    let reason = reason.expect("startup fails");
    assert!(reason.contains("could not be bound"), "{reason}");
    assert!(!reason.contains("the closure ran"), "{reason}");
}

// spec: SEC#withheld-values
// spec: SEC#matching
#[test]
fn a_shell_failure_masks_a_secret_param_it_quotes() {
    let script = format!(
        r#"{TOOLS_JOB} app.on_shell("tools", |rt, shell, p| {{ throw `bad token ${{p.token}}`; }}, #{{
            params: #{{ "token": #{{ kind: "text", secret: true }} }},
        }});"#
    );
    let reason = run_shell(
        &script,
        serde_json::json!({ "token": "Tui-Harbour-58" }),
        &FakeHost::answering(Ok(ShellExit::ended())),
        None,
    )
    .reason
    .expect("the session fails");
    assert!(reason.contains("bad token <redacted>"), "{reason}");
    assert!(!reason.contains("Tui-Harbour-58"), "{reason}");
}

// spec: ACN#param-delivery
#[test]
fn params_a_shell_declares_no_schema_for_still_reach_its_closure() {
    let oi = TestOi::new();
    let mut submitted = serde_json::Map::new();
    submitted.insert("greeting".to_owned(), serde_json::json!("hello"));
    let params = super::session::validate_shell_params(
        &oi.state,
        &std::collections::BTreeMap::new(),
        submitted,
    )
    .unwrap();
    assert_eq!(
        params.get("greeting").unwrap(),
        "hello",
        "an undeclared param was dropped"
    );
}

const SHELL_JOB: &str = "docker.io/library/busybox:1.37";

/// The records of the dynamic resources a session created.
fn session_records(ran: &ShellRan) -> Vec<crate::runtime::desired::DynamicResourceRecord> {
    let id = ran.session_id.to_string();
    ran.oi.state.db.call(move |db| {
        crate::runtime::desired::list_dynamic_resources_for_operation(db, &id).unwrap()
    })
}

/// What the session has placed in its app's desired state, as the reconciler
/// reads it.
fn session_desired(ran: &ShellRan) -> Vec<crate::runtime::DesiredResource> {
    let slot = ran.slot.read();
    let progress: Vec<_> = slot.iter().collect();
    let mut state = crate::runtime::DesiredState::default();
    crate::runtime::desired::merge_sessions(&mut state, &progress, false);
    state.resources
}

// spec: OPS#shell-sessions
// spec: RCN#desired-state
// A shell closure ran with no database and no progress, so what it started
// was in no desired state.
#[test]
fn what_a_shell_closure_starts_is_desired_and_recorded_against_the_session() {
    let script = format!(
        r#"{TOOLS_JOB} app.on_shell("tools", |rt, shell, p| {{
            shell.attach(tools);
            rt.start(app.job().image("{SHELL_JOB}").command("true"));
        }});"#
    );
    let ran = run_shell_session(
        &script,
        serde_json::json!({}),
        &FakeHost::answering(Ok(ShellExit::exited(0))),
        None,
        |_| {},
    );
    assert_eq!(ran.outcome.reason, None);

    let desired = session_desired(&ran);
    assert_eq!(desired.len(), 1, "the one job is desired");
    assert_eq!(desired[0].desired, crate::runtime::LifecycleState::Ready);
    assert!(desired[0].instance.name.is_none(), "it is anonymous");

    let records = session_records(&ran);
    assert_eq!(records.len(), 1, "the job is recorded against the session");
    assert_eq!(records[0].instance_id, desired[0].instance.id.to_hex());
    assert_eq!(
        records[0].creator,
        crate::runtime::desired::DynamicCreator::ShellSession
    );
}

// spec: APPS#redacted-values
// spec: SEC#matching
#[test]
fn a_dynamic_resource_description_quoting_a_secret_param_is_recorded_as_none() {
    let script = format!(
        r#"{TOOLS_JOB} app.on_shell("tools", |rt, shell, p| {{
            shell.attach(tools);
            rt.start(app.job().image("{SHELL_JOB}").command("true").description(`migrate with ${{p.token}}`));
            rt.start(app.job().image("{SHELL_JOB}").command("true").description("plain"));
        }}, #{{
            params: #{{ "token": #{{ kind: "text", secret: true }} }},
        }});"#
    );
    let ran = run_shell_session(
        &script,
        serde_json::json!({ "token": "Tui-Harbour-58" }),
        &FakeHost::answering(Ok(ShellExit::exited(0))),
        None,
        |_| {},
    );
    assert_eq!(ran.outcome.reason, None);
    let mut descriptions: Vec<_> = session_records(&ran)
        .into_iter()
        .map(|r| r.description)
        .collect();
    descriptions.sort();
    assert_eq!(descriptions, [None, Some("plain".to_owned())]);
}

// spec: RT#starting-resources
#[test]
fn a_shell_closure_starting_a_declared_resource_leaves_the_desired_state_alone() {
    let script = format!(
        r#"{TOOLS_JOB} app.on_shell("tools", |rt, shell, p| {{
            shell.attach(tools);
            rt.start(tools);
        }});"#
    );
    let ran = run_shell_session(
        &script,
        serde_json::json!({}),
        &FakeHost::answering(Ok(ShellExit::exited(0))),
        None,
        |_| {},
    );
    assert_eq!(ran.outcome.reason, None, "the call returned a Started");
    assert!(
        session_desired(&ran).is_empty(),
        "the declared job was not placed in the desired state"
    );
    assert!(session_records(&ran).is_empty(), "and nothing was recorded");
}

// spec: RT#stopping-resources
#[test]
fn a_shell_closure_stopping_a_declared_resource_throws() {
    let script = format!(
        r#"{TOOLS_JOB} app.on_shell("tools", |rt, shell, p| {{
            shell.attach(tools);
            rt.stop(tools);
        }});"#
    );
    let ran = run_shell_session(
        &script,
        serde_json::json!({}),
        &FakeHost::answering(Ok(ShellExit::exited(0))),
        None,
        |_| {},
    );
    let reason = ran.outcome.reason.clone().expect("the session failed");
    assert!(reason.contains("declared by the app"), "{reason}");
    assert!(session_desired(&ran).is_empty());
}

// spec: OPS#shell-sessions
// The record is all that finds a resource again after a restart, so one that
// cannot be written is not started.
#[test]
fn a_shell_resource_that_cannot_be_recorded_is_not_started() {
    let script = format!(
        r#"{TOOLS_JOB} app.on_shell("tools", |rt, shell, p| {{
            shell.attach(tools);
            rt.start(app.job().image("{SHELL_JOB}").command("true"));
        }});"#
    );
    let ran = run_shell_session(
        &script,
        serde_json::json!({}),
        &FakeHost::answering(Ok(ShellExit::exited(0))),
        None,
        |oi| {
            oi.state.db.call(|db| {
                db.conn.execute("DROP TABLE dynamic_resources", []).unwrap();
            });
        },
    );
    let reason = ran.outcome.reason.clone().expect("the session failed");
    assert!(reason.contains("could not record"), "{reason}");
    assert!(
        session_desired(&ran).is_empty(),
        "nothing was placed in the desired state"
    );
}

// spec: OPS#shell-sessions
// spec: FLT#app-faults
// The session's resources are torn down after it ends, by the same cleanup an
// operation's are, holding `operation_cleanup_stalled` for the session.
#[test]
fn a_session_whose_resources_are_slow_to_go_holds_the_stall_fault_by_its_id() {
    let oi = TestOi::with_app("demo");
    let session_id = SessionId::generate();
    let owner = session_id.to_string();
    let job = crate::runtime::identity::ResourceInstance::new_anonymous(
        "demo".parse().unwrap(),
        crate::defs::resource::ResourceKind::Job,
    );
    let (recorded, owner_for_record) = (job.clone(), owner.clone());
    oi.state.db.call(move |db| {
        crate::runtime::desired::insert_dynamic_resource(
            db,
            &recorded,
            &owner_for_record,
            crate::runtime::desired::DynamicCreator::ShellSession,
            None,
        )
        .unwrap();
        for kind in ["container_running", "container_exited", "container_removed"] {
            crate::runtime::history::insert_observation(
                db,
                &recorded,
                kind,
                &serde_json::json!({}),
            )
            .unwrap();
        }
    });
    let network = format!("seedling-{}", job.display_name);
    oi.block_on(oi.state.driver.container.create_network(
        &network,
        "fd00:0:0:1::/64".parse().unwrap(),
        None,
    ))
    .unwrap();
    let (sessions, tick_notify) = {
        let reg = oi.state.registry.read();
        let entry = reg.get("demo").unwrap();
        (Arc::clone(&entry.sessions), Arc::clone(&entry.tick_notify))
    };
    let slot = sessions.open(session_id);

    let stalled = || {
        oi.state.db.call(|db| {
            crate::runtime::faults::list_active_faults(db, None)
                .unwrap()
                .into_iter()
                .filter(|f| f.kind == crate::runtime::faults::OPERATION_CLEANUP_STALLED)
                .map(|f| f.subject)
                .collect::<Vec<_>>()
        })
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(async {
            let teardown = super::session::tear_down_session_resources(
                Arc::clone(&oi.state),
                "demo".parse().unwrap(),
                session_id,
                Arc::clone(&sessions),
                Arc::clone(&slot),
                tick_notify,
            );
            tokio::pin!(teardown);
            tokio::select! {
                () = &mut teardown => panic!("the teardown ended while the network stood"),
                () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
            }
            assert!(stalled().is_empty(), "not within the first minute");
            assert!(!sessions.is_empty(), "the slot stands while it tears down");
            tokio::select! {
                () = &mut teardown => panic!("the teardown ended while the network stood"),
                () = tokio::time::sleep(std::time::Duration::from_secs(600)) => {}
            }
            assert_eq!(stalled(), vec![owner.clone()], "held past a minute");

            oi.state
                .driver
                .container
                .remove_network(&network)
                .await
                .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(600), teardown)
                .await
                .expect("the teardown ends once the network is gone");
        });
    assert!(stalled().is_empty(), "cleared once torn down");
    assert!(sessions.is_empty(), "the slot is let go");
    assert!(
        oi.state
            .db
            .call(move |db| {
                crate::runtime::desired::list_dynamic_resources_for_operation(db, &owner).unwrap()
            })
            .is_empty(),
        "the record is forgotten"
    );
}

// spec: OPS#shell-sessions
// The slot was let go only on the session's ordinary end, so a session task
// that panicked or was dropped kept its app from ever being steady again.
#[test]
fn a_session_dropped_mid_run_still_lets_go_of_its_slot() {
    let oi = TestOi::with_app("demo");
    let (sessions, tick_notify) = {
        let reg = oi.state.registry.read();
        let entry = reg.get("demo").unwrap();
        (Arc::clone(&entry.sessions), Arc::clone(&entry.tick_notify))
    };
    let session_id = SessionId::generate();
    let slot = sessions.open(session_id);
    let teardown = super::session::SessionTeardown {
        state: Arc::clone(&oi.state),
        app_name: "demo".parse().unwrap(),
        session_id,
        sessions: Arc::clone(&sessions),
        slot,
        tick_notify,
    };

    oi.block_on(async {
        let running = tokio::spawn(async move {
            let _teardown = teardown;
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        running.abort();
        let _ = running.await;
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !sessions.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the slot is let go");
    });
}

// spec: RT#resources-a-gate-holds
#[test]
fn a_shell_closure_leaves_out_what_a_gate_came_to_hold_after_the_session_opened() {
    use crate::{
        defs::resource::{ResourceId, ResourceKind},
        runtime::{history::insert_observation, identity::ResourceInstance},
    };

    let script = format!(
        r#"{TOOLS_JOB}
        let api = app.deployment("api").image("docker.io/library/nginx:1");
        app.gate("migrate").key(1).exclusive(api);
        app.deployment("plain").image("docker.io/library/nginx:1");
        app.on_shell("tools", |rt, shell, p| {{
            rt.start(app.job().image("{SHELL_JOB}").command("true")).terminated();
            rt.start(app);
        }});"#
    );
    let ran = run_shell_session(
        &script,
        serde_json::json!({}),
        &FakeHost::answering(Ok(ShellExit::ended())),
        Some(std::time::Duration::from_secs(20)),
        |oi| {
            let db = oi.state.db.clone();
            std::thread::spawn(move || {
                let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
                let first_call = loop {
                    let found = db.call(|db| {
                        db.conn
                            .query_row(
                                "SELECT resources FROM action_log WHERE app = 'demo' AND call_index = 0",
                                [],
                                |row| row.get::<_, String>(0),
                            )
                            .ok()
                    });
                    if let Some(found) = found {
                        break found;
                    }
                    assert!(std::time::Instant::now() < until, "the closure never began");
                    std::thread::sleep(std::time::Duration::from_millis(10));
                };
                let job: Vec<ResourceInstance> = serde_json::from_str(&first_call).unwrap();
                db.call(move |db| {
                    let api = ResourceId {
                        kind: ResourceKind::Deployment,
                        name: Arc::new("api".to_owned()),
                    };
                    crate::runtime::gates::store::set_hold(
                        db,
                        &"demo".parse().unwrap(),
                        &"migrate".parse().unwrap(),
                        &[api].into(),
                    )
                    .unwrap();
                    insert_observation(
                        db,
                        &job[0],
                        "container_exited",
                        &serde_json::json!({ "exit_code": 0 }),
                    )
                    .unwrap();
                });
            });
        },
    );

    let id = ran.session_id.to_string();
    let log = ran.oi.state.db.call(move |db| {
        crate::runtime::history::load_action_log(db, &crate::runtime::barrier::OperationId(id))
            .unwrap()
    });
    let mut started: Vec<_> = log
        .get(1)
        .expect("the closure went on to its second call")
        .resources
        .iter()
        .filter_map(|resource| resource.name.clone())
        .collect();
    started.sort();
    assert_eq!(
        started,
        vec!["plain", "tools"],
        "api is held by a gate that held it only after the session opened"
    );
}

// spec: SHL#opening-an-app-shell
// spec: ACN#param-delivery
#[test]
fn a_shell_param_takes_its_native_json_type_and_reaches_the_closure_parsed() {
    use crate::defs::param_def::{ParamDef, ParamKind};

    let oi = TestOi::new();
    let schema = std::collections::BTreeMap::from([(
        seedling_protocol::names::ParamName::new("verbose").unwrap(),
        ParamDef {
            kind: ParamKind::Boolean,
            required: true,
            default_value: None,
            description: None,
            secret: None,
        },
    )]);
    let mut submitted = serde_json::Map::new();
    submitted.insert("verbose".to_owned(), serde_json::json!(true));
    let params = super::session::validate_shell_params(&oi.state, &schema, submitted).unwrap();
    assert!(params.to_rhai().unwrap()["verbose"].as_bool().unwrap());

    let mut submitted = serde_json::Map::new();
    submitted.insert("verbose".to_owned(), serde_json::json!(1));
    let err = super::session::validate_shell_params(&oi.state, &schema, submitted)
        .expect_err("an integer is not a boolean");
    assert!(
        err.message.contains("an integer is given"),
        "{}",
        err.message
    );
}
