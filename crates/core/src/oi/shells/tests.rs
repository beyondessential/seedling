use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use seedling_protocol::{actor::Actor, events::EventSenderWithActor, names::SessionId};
use tokio::io::AsyncWrite;

use super::{
    registry::{ShellSession, StopSignal},
    session::{
        LiveShell, SessionIo, SessionOutcome, SessionProgress, SessionStreams, run_app_shell,
        run_to_exit, session_outcome,
    },
};
use crate::{
    oi::{
        state::OiState,
        test_support::{Loopback, TestOi, loopback},
    },
    runtime::{
        apps::SessionSlot,
        barrier::shell::{ShellExecTarget, ShellExit, ShellHost},
    },
    system::types::ExecHandle,
};

/// Opens a session stream the way the daemon sees one, returning the server's
/// streams for the session and the client's read side of the session stream.
async fn open_streams(link: &Loopback) -> (SessionIo, quinn::RecvStream) {
    let (mut client_send, client_recv) = link.client.open_bi().await.expect("open bi");
    client_send.write_all(b"\n").await.expect("first bytes");
    let (send, server_recv) = link.server.accept_bi().await.expect("accept bi");
    let stdout = link.server.open_uni().await.expect("stdout");
    let stderr = link.server.open_uni().await.expect("stderr");
    // Keep the client's write side open for as long as the test lives: a
    // session whose client closed its stdin is a different session end.
    std::mem::forget(client_send);
    (
        SessionIo {
            recv: server_recv,
            leftover_stdin: Vec::new(),
            streams: SessionStreams {
                send,
                stdout,
                stderr,
            },
        },
        client_recv,
    )
}

/// A stdin whose every write fails, as when the job's pty has already gone.
struct BrokenStdin;

impl AsyncWrite for BrokenStdin {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Session events bound to a stand-in actor.
fn events(state: &crate::oi::state::OiState) -> EventSenderWithActor {
    EventSenderWithActor::new(state.event_tx.clone(), Arc::new(Actor::default()))
}

fn process_exists(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process can be signalled.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// A session as it stands once its job has started: registered, with a job
/// that never exits on its own and a pod network of its own.
struct Started {
    session_id: SessionId,
    stop: StopSignal,
    live: LiveShell,
    pid: u32,
    /// The job's end of its output. Dropping it ends the output, which ends
    /// the session, so a test holds it for as long as the job is meant to live.
    output: tokio::io::DuplexStream,
}

async fn started(state: &OiState, stdin: impl AsyncWrite + Send + Unpin + 'static) -> Started {
    let session_id = SessionId::generate();
    let stop = StopSignal::new();
    state.shells.insert(ShellSession::starting(
        session_id,
        "app".parse().unwrap(),
        "shell".to_owned(),
        None,
        (24, 80),
        stop.clone(),
    ));

    let net_name = "seedling-shell-test";
    state
        .container_runtime
        .create_network(net_name, "fd5e:ed11:9000:1::/64".parse().unwrap(), None)
        .await
        .expect("create network");

    let child = tokio::process::Command::new("sleep")
        .arg("60")
        .kill_on_drop(true)
        .spawn()
        .expect("spawn job");
    let pid = child.id().expect("job pid");
    let (output, job_stdout) = tokio::io::duplex(64);
    Started {
        session_id,
        stop,
        live: LiveShell {
            exec: ExecHandle {
                stdin: Box::new(stdin),
                stdout: Box::new(job_stdout),
                pty_master_fd: -1,
                child,
            },
            container_name: "shell-test".to_owned(),
            net_name: net_name.to_owned(),
        },
        pid,
        output,
    }
}

// spec: SHL#session-end
#[test]
fn stdin_failure_after_exec_removes_the_job_and_its_network() {
    let oi = TestOi::new();
    let state = Arc::clone(&oi.state);

    oi.block_on(async {
        let link = loopback().await;
        let (io, mut client_recv) = open_streams(&link).await;
        let Started {
            session_id,
            stop,
            live,
            pid,
            output: _output,
        } = started(&state, BrokenStdin).await;
        let net_name = live.net_name.clone();

        let exit_code = run_to_exit(
            &state,
            &events(&state),
            session_id,
            &stop,
            live,
            SessionIo {
                leftover_stdin: b"typed before the shell opened".to_vec(),
                ..io
            },
        )
        .await;

        assert_eq!(exit_code, -1);
        assert!(
            !state
                .container_runtime
                .network_exists(&net_name)
                .await
                .unwrap(),
            "the session's pod network is removed"
        );
        assert!(!process_exists(pid), "the job is terminated and reaped");
        assert!(state.shells.list(None).is_empty());

        let frame = client_recv.read_to_end(1024).await.expect("exit frame");
        assert_eq!(frame, b"{\"exit_code\":-1}\n");
    });
}

// spec: SHL#session-startup
#[test]
fn stop_that_lands_before_the_job_attaches_ends_the_job() {
    let oi = TestOi::new();
    let state = Arc::clone(&oi.state);

    oi.block_on(async {
        let link = loopback().await;
        let (io, mut client_recv) = open_streams(&link).await;
        let Started {
            session_id,
            stop,
            live,
            pid,
            output: _output,
        } = started(&state, tokio::io::sink()).await;
        let net_name = live.net_name.clone();

        // The operator stops the session while its job is still starting, so
        // the registry entry is gone by the time the job is ready to attach.
        assert!(state.shells.stop(&session_id));

        let exit_code = run_to_exit(&state, &events(&state), session_id, &stop, live, io).await;

        assert_eq!(exit_code, -1);
        assert!(!process_exists(pid), "the job does not run on");
        assert!(
            !state
                .container_runtime
                .network_exists(&net_name)
                .await
                .unwrap()
        );

        let frame = client_recv.read_to_end(1024).await.expect("exit frame");
        assert_eq!(frame, b"{\"exit_code\":-1}\n");
    });
}

// spec: SHL#session-end
#[test]
fn stop_after_the_job_is_running_ends_the_session() {
    let oi = TestOi::new();
    let state = Arc::clone(&oi.state);

    oi.block_on(async {
        let link = loopback().await;
        let (io, mut client_recv) = open_streams(&link).await;
        let Started {
            session_id,
            stop,
            live,
            pid,
            output: _output,
        } = started(&state, tokio::io::sink()).await;

        let stopper = {
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                state.shells.stop(&session_id)
            })
        };
        let exit_code = run_to_exit(&state, &events(&state), session_id, &stop, live, io).await;

        assert!(stopper.await.unwrap(), "the session was registered to stop");
        assert_eq!(
            exit_code,
            128 + libc::SIGTERM,
            "the job's signal exit status"
        );
        assert!(!process_exists(pid));
        let frame = client_recv.read_to_end(1024).await.expect("exit frame");
        assert_eq!(frame, b"{\"exit_code\":143}\n");
    });
}

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
    slot: SessionSlot,
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
    run_shell_session(script, params, host, stop_after).outcome
}

/// As [`run_shell`], keeping the app and the session's slot for inspection.
fn run_shell_session(
    script: &str,
    params: serde_json::Value,
    host: &Arc<FakeHost>,
    stop_after: Option<std::time::Duration>,
) -> ShellRan {
    use seedling_protocol::names::ShellName;

    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        serde_json::json!({ "app": "demo", "script": script }),
    )
    .unwrap();
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
    let params = params.as_object().cloned().unwrap_or_default();
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

// spec: ACN#param-delivery
#[test]
fn params_a_shell_declares_no_schema_for_still_reach_its_closure() {
    let oi = TestOi::new();
    let submitted = std::collections::BTreeMap::from([("greeting".to_owned(), "hello".to_owned())]);
    let params = super::session::validate_shell_params(
        &oi.state,
        &std::collections::BTreeMap::new(),
        &submitted,
    )
    .unwrap();
    assert_eq!(
        params["greeting"], "hello",
        "an undeclared param was dropped"
    );
}
