use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use rustls_pki_types::PrivatePkcs8KeyDer;
use seedling_protocol::{actor::Actor, events::EventSenderWithActor, names::SessionId};
use tokio::io::AsyncWrite;

use super::{
    registry::{ShellSession, StopSignal},
    session::{LiveShell, SessionStreams, run_to_exit},
};
use crate::{
    oi::{state::OiState, test_support::TestOi},
    system::types::ExecHandle,
};

/// A connected QUIC pair over loopback. The server side is what a shell session
/// runs on, the client side is the operator's end of it.
struct Loopback {
    server: quinn::Connection,
    client: quinn::Connection,
    _endpoints: (quinn::Endpoint, quinn::Endpoint),
}

async fn loopback() -> Loopback {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("cert");
    let key = PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
    let server_config =
        quinn::ServerConfig::with_single_cert(vec![cert.cert.der().clone()], key.into())
            .expect("server config");
    let server_endpoint =
        quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap()).expect("server");
    let addr = server_endpoint.local_addr().expect("addr");

    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.cert.der().clone()).expect("root");
    let mut client_endpoint =
        quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).expect("client");
    client_endpoint.set_default_client_config(
        quinn::ClientConfig::with_root_certificates(Arc::new(roots)).expect("client config"),
    );

    let accept = {
        let endpoint = server_endpoint.clone();
        tokio::spawn(async move { endpoint.accept().await.expect("incoming").await })
    };
    let client = client_endpoint
        .connect(addr, "localhost")
        .expect("connect")
        .await
        .expect("client connection");
    let server = accept.await.unwrap().expect("server connection");

    Loopback {
        server,
        client,
        _endpoints: (server_endpoint, client_endpoint),
    }
}

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
