//! A session's job as the daemon runs it: a real process, ended by signal.

use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use seedling_protocol::{actor::Actor, events::EventSenderWithActor, names::SessionId};
use tokio::io::AsyncWrite;

use super::super::{
    registry::{ShellSession, StopSignal},
    session::{LiveShell, SessionIo, SessionStreams, run_to_exit},
};
use crate::{
    oi::{
        state::OiState,
        test_support::{Loopback, TestOi, loopback},
    },
    system::{
        System,
        backends::Backends,
        session_mounts::{
            SessionMount,
            tests::{backends, tools_mounts},
        },
        stub::{
            SessionCall, StubContainerRuntime, StubDataPlane, StubNetworkProxy, StubProcessManager,
        },
        types::ExecHandle,
    },
};

/// Opens a session stream the way the daemon sees one, returning the server's
/// streams for the session and the client's read side of the session stream.
pub(in crate::oi::shells) async fn open_streams(link: &Loopback) -> (SessionIo, quinn::RecvStream) {
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

/// A harness on stub backends, with the data plane and container runtime kept
/// for inspection.
pub(in crate::oi::shells) struct StubRig {
    pub(in crate::oi::shells) oi: TestOi,
    pub(in crate::oi::shells) plane: Arc<StubDataPlane>,
    pub(in crate::oi::shells) container: Arc<StubContainerRuntime>,
    pub(in crate::oi::shells) _dir: tempfile::TempDir,
}

pub(in crate::oi::shells) fn stub_rig() -> StubRig {
    let dir = tempfile::tempdir().unwrap();
    let container = Arc::new(StubContainerRuntime::new(dir.path().join("vols")));
    let process = Arc::new(StubProcessManager::new(Arc::clone(&container)));
    let plane = Arc::new(StubDataPlane::default());
    let mut system = System::for_tests(
        Backends::working(Arc::clone(&container) as _, process),
        Arc::new(StubNetworkProxy),
        dir.path(),
    );
    system.data_plane = Arc::clone(&plane) as _;
    StubRig {
        oi: TestOi::with_driver(Arc::new(system)),
        plane,
        container,
        _dir: dir,
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
    let net_prefix = "fd5e:ed11:9000:1::/64".parse().unwrap();
    state
        .container_runtime
        .create_network(
            net_name,
            &crate::system::pod_bridge_name(&net_prefix),
            net_prefix,
            None,
        )
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

// spec: INFRA#mount-endpoint-addressing
#[test]
fn the_end_of_a_running_session_removes_its_mount_translations() {
    let StubRig {
        oi, plane, _dir, ..
    } = stub_rig();
    let state = Arc::clone(&oi.state);

    oi.block_on(async {
        let link = loopback().await;
        let (io, _client_recv) = open_streams(&link).await;
        let Started {
            session_id,
            stop,
            live,
            output,
            ..
        } = started(&state, tokio::io::sink()).await;
        let net_name = live.net_name.clone();
        state
            .driver
            .session_mounts
            .follow(
                &*state.driver.data_plane,
                &state.db,
                backends(&["fd5e:ed11:9000:4001::2"]),
            )
            .await;
        state
            .driver
            .session_mounts
            .register(
                &*state.driver.data_plane,
                &state.db,
                &net_name,
                SessionMount {
                    app: "demo".parse().unwrap(),
                    prefix: "fd5e:ed11:9000:1::/64".parse().unwrap(),
                    mounts: tools_mounts(".mount(db.port(5432))"),
                },
            )
            .await
            .unwrap();
        assert!(plane.session_mounts().contains_key(&net_name));

        // The job's output ending ends the session, as the client going does.
        drop(output);
        run_to_exit(&state, &events(&state), session_id, &stop, live, io).await;

        assert_eq!(
            plane.session_calls().last(),
            Some(&SessionCall::Remove(net_name.clone()))
        );
        assert!(plane.session_mounts().is_empty());
    });
}
