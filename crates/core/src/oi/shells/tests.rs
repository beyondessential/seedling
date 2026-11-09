use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use rustls_pki_types::PrivatePkcs8KeyDer;
use seedling_protocol::names::SessionId;
use tokio::io::AsyncWrite;

use super::session::{LiveShell, SessionStreams, run_to_exit};
use crate::{oi::test_support::TestOi, system::types::ExecHandle};

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

fn process_exists(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process can be signalled.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

// i[verify shell.cleanup]
#[test]
fn stdin_failure_after_exec_removes_the_job_and_its_network() {
    let oi = TestOi::new();
    let state = Arc::clone(&oi.state);

    oi.block_on(async {
        let link = loopback().await;
        let (streams, mut recv, mut client_recv) = open_streams(&link).await;

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
        let live = LiveShell {
            exec: ExecHandle {
                stdin: Box::new(BrokenStdin),
                stdout: Box::new(tokio::io::empty()),
                pty_master_fd: -1,
                child,
            },
            container_name: "shell-test".to_owned(),
            net_name: net_name.to_owned(),
        };

        let (_stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
        let exit_code = run_to_exit(
            &state,
            SessionId::generate(),
            live,
            &mut recv,
            b"typed before the shell opened",
            &mut stop_rx,
            streams,
        )
        .await;

        assert_eq!(exit_code, -1);
        assert!(
            !state
                .container_runtime
                .network_exists(net_name)
                .await
                .unwrap(),
            "the session's pod network is removed"
        );
        assert!(!process_exists(pid), "the job is terminated and reaped");

        let frame = client_recv.read_to_end(1024).await.expect("exit frame");
        assert_eq!(frame, b"{\"exit_code\":-1}\n");
    });
}
