//! The byte relay both ends of a TCP port forward run between a forward data
//! stream and a TCP connection.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Application error code a forward data stream is reset with when its
/// tunneled connection fails.
pub const FORWARD_ABORTED: quinn::VarInt = quinn::VarInt::from_u32(0);

// spec: FWD#tcp-tunnelling
// spec: WIRE#port-forward-data
/// Relay a forward data stream (past its header) and a TCP connection until
/// both directions have finished.
///
/// `to_stream` and `from_stream` are told the size of each chunk delivered in
/// that direction. On failure the stream is reset and stopped rather than
/// finished, so the far end does not read a truncated transfer as whole.
pub async fn relay_forward_stream<R, W>(
    (stream_send, stream_recv): (&mut quinn::SendStream, &mut quinn::RecvStream),
    (local_read, local_write): (&mut R, &mut W),
    to_stream: impl Fn(usize),
    from_stream: impl Fn(usize),
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let result = relay(
        (local_read, stream_send),
        (stream_recv, local_write),
        to_stream,
        from_stream,
    )
    .await;
    if result.is_err() {
        abort_forward_stream(stream_send, stream_recv);
    }
    result
}

/// Reset and stop a forward data stream whose tunneled connection failed, or
/// could not be opened.
pub fn abort_forward_stream(send: &mut quinn::SendStream, recv: &mut quinn::RecvStream) {
    let _ = send.reset(FORWARD_ABORTED);
    let _ = recv.stop(FORWARD_ABORTED);
}

/// Copy two directions concurrently, each on its own: a write stalled in one
/// never stops the other being read. A direction whose source ends finishes
/// its sink and leaves the other running, so a half-close carries through.
/// The first error in either direction ends both.
async fn relay<AR, AW, BR, BW>(
    (a_read, a_write): (&mut AR, &mut AW),
    (b_read, b_write): (&mut BR, &mut BW),
    a_counted: impl Fn(usize),
    b_counted: impl Fn(usize),
) -> io::Result<()>
where
    AR: AsyncRead + Unpin,
    AW: AsyncWrite + Unpin,
    BR: AsyncRead + Unpin,
    BW: AsyncWrite + Unpin,
{
    tokio::try_join!(
        pump(a_read, a_write, a_counted),
        pump(b_read, b_write, b_counted)
    )?;
    Ok(())
}

async fn pump<R, W>(read: &mut R, write: &mut W, counted: impl Fn(usize)) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 8192];
    loop {
        let n = read.read(&mut buf).await?;
        if n == 0 {
            return write.shutdown().await;
        }
        write.write_all(&buf[..n]).await?;
        counted(n);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::{DuplexStream, duplex, split};

    use super::*;

    /// Two peers joined through the relay over in-memory pipes, as a forward
    /// joins its client and its service. The service's pipe is smaller than
    /// one relayed chunk, so a write toward it waits on the service reading.
    fn relayed() -> (
        DuplexStream,
        DuplexStream,
        tokio::task::JoinHandle<io::Result<()>>,
    ) {
        let (client, client_side) = duplex(64 * 1024);
        let (service, service_side) = duplex(64);
        let task = tokio::spawn(async move {
            let (mut cr, mut cw) = split(client_side);
            let (mut sr, mut sw) = split(service_side);
            relay((&mut cr, &mut sw), (&mut sr, &mut cw), |_| {}, |_| {}).await
        });
        (client, service, task)
    }

    // spec: FWD#tcp-tunnelling
    #[tokio::test]
    async fn half_closed_client_still_receives_the_response() {
        let (mut client, mut service, task) = relayed();
        let response = vec![7u8; 10_000];

        let client_side = async {
            client.write_all(b"request").await.unwrap();
            client.shutdown().await.unwrap();
            let mut received = Vec::new();
            client.read_to_end(&mut received).await.unwrap();
            received
        };
        let service_side = async {
            let mut request = Vec::new();
            service.read_to_end(&mut request).await.unwrap();
            service.write_all(&response).await.unwrap();
            service.shutdown().await.unwrap();
            request
        };
        let (received, request) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(client_side, service_side)
        })
        .await
        .expect("relay stalled");

        assert_eq!(request, b"request");
        assert_eq!(received, response);
        task.await.unwrap().unwrap();
    }

    // spec: FWD#tcp-tunnelling
    #[tokio::test]
    async fn a_stalled_direction_does_not_hold_up_the_other() {
        let (mut client, mut service, task) = relayed();
        let payload = vec![1u8; 256 * 1024];

        // An echo service that writes each chunk back before reading the next:
        // a relay serving one direction at a time ends up blocked writing to
        // it while it is blocked writing back to the relay.
        let echo = async {
            let mut buf = vec![0u8; 4096];
            loop {
                let n = service.read(&mut buf).await.unwrap();
                if n == 0 {
                    service.shutdown().await.unwrap();
                    break;
                }
                service.write_all(&buf[..n]).await.unwrap();
            }
        };
        let (mut cr, mut cw) = split(&mut client);
        let send = async {
            cw.write_all(&payload).await.unwrap();
            cw.shutdown().await.unwrap();
        };
        let receive = async {
            let mut got = Vec::new();
            cr.read_to_end(&mut got).await.unwrap();
            got.len()
        };
        let ((), (), echoed) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(echo, send, receive)
        })
        .await
        .expect("relay deadlocked");
        assert_eq!(echoed, payload.len());
        task.await.unwrap().unwrap();
    }
}
