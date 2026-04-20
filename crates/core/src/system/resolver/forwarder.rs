//! Tiny UDP + TCP DNS forwarder used to bridge the CoreDNS resolver
//! container to the host's `systemd-resolved` "extra" stub at `127.0.0.54`.
//!
//! CoreDNS runs inside a container and cannot reach `127.0.0.53` /
//! `127.0.0.54` on the host's loopback directly. To preserve
//! systemd-resolved features (split DNS, Tailscale MagicDNS, DNSSEC,
//! per-link resolvers, search domains), the daemon runs this forwarder
//! on the resolver bridge gateway IP. CoreDNS points its `forward`
//! plugin at that address and each query is proxied to a host-side
//! upstream (typically `127.0.0.54:53`).
//!
//! This module is only started when the operator has not provided
//! `--dns-upstreams` at the CLI; explicit upstreams bypass the
//! forwarder entirely and are written straight into the Corefile.

use std::{net::SocketAddr, time::Duration};

use tokio::{
    io::copy_bidirectional,
    net::{TcpListener, TcpStream, UdpSocket},
    task::JoinHandle,
    time,
};
use tracing::{debug, trace, warn};

/// Maximum size of a single UDP DNS datagram we'll forward. 4 KiB
/// comfortably covers EDNS0 buffers used in practice (the Corefile sets
/// `forward`'s default buffer, typically ≤4096) without paying for large
/// allocations per query.
const UDP_BUFFER_BYTES: usize = 4096;

/// Per-query timeout waiting for an upstream reply before the
/// ephemeral forwarding socket is dropped.
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(5);

/// Backoff between bind attempts while waiting for the resolver bridge
/// interface to be created by netavark on the first reconcile tick.
const BIND_RETRY_DELAY: Duration = Duration::from_secs(1);

/// Spawn the DNS forwarder. Returns a join handle; the caller keeps it
/// alive for the daemon's lifetime (handles dropped before task exit
/// abort the task).
///
/// `listen` is typically `[<resolver-bridge-gw>]:53`; `upstream` is the
/// host-side resolver (typically `127.0.0.54:53`, the systemd-resolved
/// extra stub which, unlike `127.0.0.53`, accepts queries whose source
/// address is not in `127.0.0.0/8`).
// r[impl infra.resolver.upstreams]
pub fn spawn_dns_forwarder(listen: SocketAddr, upstream: SocketAddr) -> JoinHandle<()> {
    tokio::spawn(async move {
        let udp = bind_udp_with_retry(listen).await;
        let tcp = bind_tcp_with_retry(listen).await;
        debug!(%listen, %upstream, "DNS forwarder ready");

        tokio::join!(udp_loop(udp, upstream), tcp_loop(tcp, upstream));
    })
}

async fn bind_udp_with_retry(addr: SocketAddr) -> UdpSocket {
    loop {
        match UdpSocket::bind(addr).await {
            Ok(sock) => return sock,
            Err(e) => {
                trace!(%addr, error = %e, "DNS forwarder UDP bind failed, retrying");
                time::sleep(BIND_RETRY_DELAY).await;
            }
        }
    }
}

async fn bind_tcp_with_retry(addr: SocketAddr) -> TcpListener {
    loop {
        match TcpListener::bind(addr).await {
            Ok(l) => return l,
            Err(e) => {
                trace!(%addr, error = %e, "DNS forwarder TCP bind failed, retrying");
                time::sleep(BIND_RETRY_DELAY).await;
            }
        }
    }
}

async fn udp_loop(listener: UdpSocket, upstream: SocketAddr) {
    let listener = std::sync::Arc::new(listener);
    let mut buf = vec![0u8; UDP_BUFFER_BYTES];
    loop {
        let (n, client_addr) = match listener.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "DNS forwarder UDP recv failed");
                continue;
            }
        };
        let query = buf[..n].to_vec();
        let listener = std::sync::Arc::clone(&listener);
        tokio::spawn(async move {
            if let Err(e) = forward_udp_query(&listener, client_addr, upstream, &query).await {
                trace!(%client_addr, error = %e, "DNS forwarder UDP query failed");
            }
        });
    }
}

async fn forward_udp_query(
    listener: &UdpSocket,
    client_addr: SocketAddr,
    upstream: SocketAddr,
    query: &[u8],
) -> std::io::Result<()> {
    let unspecified: SocketAddr = if upstream.is_ipv6() {
        "[::]:0".parse().unwrap()
    } else {
        "0.0.0.0:0".parse().unwrap()
    };
    let out = UdpSocket::bind(unspecified).await?;
    out.connect(upstream).await?;
    out.send(query).await?;

    let mut reply = vec![0u8; UDP_BUFFER_BYTES];
    let n = time::timeout(UPSTREAM_TIMEOUT, out.recv(&mut reply))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "upstream timeout"))??;
    listener.send_to(&reply[..n], client_addr).await?;
    Ok(())
}

async fn tcp_loop(listener: TcpListener, upstream: SocketAddr) {
    loop {
        let (client, client_addr) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "DNS forwarder TCP accept failed");
                time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        tokio::spawn(async move {
            if let Err(e) = forward_tcp_conn(client, upstream).await {
                trace!(%client_addr, error = %e, "DNS forwarder TCP conn failed");
            }
        });
    }
}

async fn forward_tcp_conn(mut client: TcpStream, upstream: SocketAddr) -> std::io::Result<()> {
    let mut up = TcpStream::connect(upstream).await?;
    copy_bidirectional(&mut client, &mut up).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    /// A trivial UDP DNS "server" that replies to every datagram with a
    /// fixed payload, used to verify that the forwarder proxies queries
    /// and delivers replies back to the correct client.
    async fn echo_upstream(payload: Vec<u8>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let sock = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .unwrap();
        let addr = sock.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut buf = [0u8; 512];
            loop {
                let Ok((_n, from)) = sock.recv_from(&mut buf).await else {
                    return;
                };
                let _ = sock.send_to(&payload, from).await;
            }
        });
        (addr, handle)
    }

    // r[verify infra.resolver.upstreams]
    #[tokio::test]
    async fn forwards_udp_query_and_returns_reply() {
        let (upstream_addr, _up) = echo_upstream(b"\xaa\xbb\xcc".to_vec()).await;

        let listen = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let probe = UdpSocket::bind(listen).await.unwrap();
        let forwarder_listen = probe.local_addr().unwrap();
        drop(probe);

        let _handle = spawn_dns_forwarder(forwarder_listen, upstream_addr);
        time::sleep(Duration::from_millis(100)).await;

        let client = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .unwrap();
        client.send_to(b"query", forwarder_listen).await.unwrap();

        let mut buf = [0u8; 512];
        let (n, _) = time::timeout(Duration::from_secs(2), client.recv_from(&mut buf))
            .await
            .expect("reply before timeout")
            .unwrap();
        assert_eq!(&buf[..n], b"\xaa\xbb\xcc");
    }

    #[tokio::test]
    async fn udp_bind_retries_when_address_missing() {
        // Bind should eventually succeed once the address is available.
        // We simulate this by binding to localhost directly; the retry
        // loop path is exercised in production when the bridge comes up
        // asynchronously. This test just verifies that a successful
        // bind returns promptly (no artificial delay on the happy path).
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let probe = UdpSocket::bind(addr).await.unwrap();
        let target = probe.local_addr().unwrap();
        drop(probe);
        let sock = time::timeout(Duration::from_secs(2), bind_udp_with_retry(target))
            .await
            .expect("bind within timeout");
        assert_eq!(sock.local_addr().unwrap(), target);
    }
}
