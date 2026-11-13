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
    session::{LiveShell, SessionStreams, run_to_exit},
};
use crate::{
    oi::{
        state::OiState,
        test_support::{Loopback, TestOi, loopback},
    },
    system::types::ExecHandle,
};

/// Opens a session stream the way the daemon sees one, returning the server's
/// streams for the session and the client's read side of the session stream.
async fn open_streams(link: &Loopback) -> (SessionStreams, quinn::RecvStream, quinn::RecvStream) {
    let (mut client_send, client_recv) = link.client.open_bi().await.expect("open bi");
    client_send.write_all(b"\n").await.expect("first bytes");
    let (send, recv) = link.server.accept_bi().await.expect("accept bi");
    let stdout = link.server.open_uni().await.expect("stdout");
    let stderr = link.server.open_uni().await.expect("stderr");
    // Keep the client's write side open for as long as the test lives: a
    // session whose client closed its stdin is a different session end.
    std::mem::forget(client_send);
    (
        SessionStreams {
            send,
            stdout,
            stderr,
        },
        recv,
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
        let (streams, mut recv, mut client_recv) = open_streams(&link).await;
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
            &mut recv,
            b"typed before the shell opened",
            streams,
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
        let (streams, mut recv, mut client_recv) = open_streams(&link).await;
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

        let exit_code = run_to_exit(
            &state,
            &events(&state),
            session_id,
            &stop,
            live,
            &mut recv,
            b"",
            streams,
        )
        .await;

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

// spec: SHL#stopping-a-session
#[test]
fn stop_after_the_job_is_running_ends_the_session() {
    let oi = TestOi::new();
    let state = Arc::clone(&oi.state);

    oi.block_on(async {
        let link = loopback().await;
        let (streams, mut recv, mut client_recv) = open_streams(&link).await;
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
        let exit_code = run_to_exit(
            &state,
            &events(&state),
            session_id,
            &stop,
            live,
            &mut recv,
            b"",
            streams,
        )
        .await;

        assert!(stopper.await.unwrap(), "the session was registered to stop");
        assert_eq!(exit_code, -1);
        assert!(!process_exists(pid));
        let frame = client_recv.read_to_end(1024).await.expect("exit frame");
        assert_eq!(frame, b"{\"exit_code\":-1}\n");
    });
}

/// Run the `tools` shell of a `demo` app declared by `script`, with `params`,
/// and say why its startup failed.
fn shell_startup_failure(script: &str, params: serde_json::Value) -> Option<String> {
    use seedling_protocol::names::ShellName;

    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        serde_json::json!({ "app": "demo", "script": script }),
    )
    .unwrap();
    let (app, schema) = {
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
        (entry.app.clone(), schema)
    };
    let params = params.as_object().cloned().unwrap_or_default();
    let failure = oi.block_on(super::session::start_app_shell(
        &oi.state,
        app,
        &"demo".parse().unwrap(),
        &"tools".parse().unwrap(),
        &schema,
        params,
        &StopSignal::new(),
    ));
    match failure {
        Ok(_) => None,
        Err(f) => f.reason,
    }
}

// spec: ACN#shell-control
#[test]
fn shell_error_delivers_its_message_to_the_client() {
    let reason = shell_startup_failure(
        r#"
        app.on_shell("tools", |rt, shell, p| { shell.error("not today"); });
    "#,
        serde_json::json!({}),
    );
    assert_eq!(reason.as_deref(), Some("not today"));
}

// spec: ACN#volume-params
#[test]
fn a_shell_whose_volume_param_cannot_be_bound_does_not_run_its_closure() {
    // The volume was validated and has since gone; the closure would have run
    // with the key missing.
    let reason = shell_startup_failure(
        r#"
        app.on_shell("tools", |rt, shell, p| { shell.error("the closure ran"); },
            #{ params: #{ data: #{ kind: "volume", required: true } } });
    "#,
        serde_json::json!({ "data": "gone-volume" }),
    );
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
