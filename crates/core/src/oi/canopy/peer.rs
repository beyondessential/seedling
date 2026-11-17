//! Opening a relay stream to an offering client.
//!
//! In production the peer is the QUIC connection the offer arrived on. The
//! trait exists so the relay can be exercised over an in-memory pipe as well,
//! and so nothing above it has to know that a relay stream is a QUIC stream.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use tokio::io::{AsyncRead, AsyncWrite};

/// The write half of a relay stream.
pub trait RelaySend: AsyncWrite + Send + Unpin {
    /// Abandon the stream's send side: what was written and not yet delivered
    /// is dropped, and the peer is told the stream was cut off rather than
    /// ended. Harmless on a stream already reset.
    fn reset(&mut self);
}

/// The read half of a relay stream.
pub trait RelayRecv: AsyncRead + Send + Unpin {
    /// Tell the peer to stop sending: the answer is no longer wanted.
    fn stop(&mut self);
}

/// The two halves of a relay stream: what the server writes, and what it reads.
///
/// A request that is given up on is abandoned with [`RelayStream::reset`],
/// which resets both directions, so the peer learns it at once and stops
/// working on an answer nobody will read. Note that abandoning cannot un-issue
/// a request the peer has already passed to Canopy.
pub struct RelayStream {
    pub send: Box<dyn RelaySend>,
    pub recv: Box<dyn RelayRecv>,
}

impl RelayStream {
    /// Reset the stream in both directions.
    // spec: RLY#relay-limits
    pub fn reset(&mut self) {
        self.send.reset();
        self.recv.stop();
    }
}

/// The application error code carried by a reset of a relay stream.
const RELAY_RESET_CODE: u32 = 0;

impl RelaySend for quinn::SendStream {
    fn reset(&mut self) {
        let _ = quinn::SendStream::reset(self, quinn::VarInt::from_u32(RELAY_RESET_CODE));
    }
}

impl RelayRecv for quinn::RecvStream {
    fn stop(&mut self) {
        let _ = quinn::RecvStream::stop(self, quinn::VarInt::from_u32(RELAY_RESET_CODE));
    }
}

// spec: WIRE#canopy-relay-streams
/// A client that has offered to carry Canopy requests.
pub trait RelayPeer: Send + Sync + 'static {
    /// Open a fresh stream for one relayed request.
    ///
    /// One stream per request is what makes concurrent requests independent:
    /// each gets its own flow control, and the end of the stream is the end of
    /// the body, so no length framing or multiplexing is needed above this.
    fn open(&self) -> BoxFuture<'_, Result<RelayStream, String>>;
}

/// The offering client's QUIC connection.
pub struct QuicPeer {
    conn: quinn::Connection,
}

impl QuicPeer {
    /// Wrap a connection as a shared peer, which is the only form an offer
    /// holds it in.
    pub fn shared(conn: quinn::Connection) -> Arc<dyn RelayPeer> {
        Arc::new(Self { conn })
    }
}

impl RelayPeer for QuicPeer {
    // spec: RLY#relay-streams
    fn open(&self) -> BoxFuture<'_, Result<RelayStream, String>> {
        Box::pin(async move {
            let (send, recv) = self
                .conn
                .open_bi()
                .await
                .map_err(|e| format!("cannot open a relay stream: {e}"))?;
            Ok(RelayStream {
                send: Box::new(send),
                recv: Box::new(recv),
            })
        })
    }
}

#[cfg(test)]
pub(super) use test_peers::{DuplexPeer, NullPeer};

#[cfg(test)]
mod test_peers {
    use super::*;

    /// A peer that never yields a stream, for registry tests that only care
    /// about bookkeeping.
    pub struct NullPeer;

    impl RelayPeer for NullPeer {
        fn open(&self) -> BoxFuture<'_, Result<RelayStream, String>> {
            Box::pin(async { Err("this peer opens no streams".to_owned()) })
        }
    }

    /// A peer backed by an in-memory pipe, handing the far end to a responder
    /// so a relayed request can be answered without a network.
    pub struct DuplexPeer {
        /// Called with the client end of each opened stream.
        responder: Box<dyn Fn(tokio::io::DuplexStream) + Send + Sync>,
        /// Whether opening should fail, to exercise the unreachable-peer path.
        fail: bool,
        /// How many times a stream this peer opened was reset.
        resets: Arc<std::sync::atomic::AtomicUsize>,
    }

    struct CountedSend {
        inner: tokio::io::WriteHalf<tokio::io::DuplexStream>,
        resets: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl AsyncWrite for CountedSend {
        fn poll_write(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
        }

        fn poll_flush(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    impl RelaySend for CountedSend {
        fn reset(&mut self) {
            self.resets
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    struct PlainRecv(tokio::io::ReadHalf<tokio::io::DuplexStream>);

    impl AsyncRead for PlainRecv {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.0).poll_read(cx, buf)
        }
    }

    impl RelayRecv for PlainRecv {
        fn stop(&mut self) {}
    }

    impl DuplexPeer {
        pub fn new(
            responder: impl Fn(tokio::io::DuplexStream) + Send + Sync + 'static,
        ) -> Arc<Self> {
            Arc::new(Self {
                responder: Box::new(responder),
                fail: false,
                resets: Arc::default(),
            })
        }

        pub fn failing() -> Arc<Self> {
            Arc::new(Self {
                responder: Box::new(|_| {}),
                fail: true,
                resets: Arc::default(),
            })
        }

        /// How many streams this peer opened have been reset.
        pub fn resets(&self) -> usize {
            self.resets.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl RelayPeer for DuplexPeer {
        fn open(&self) -> BoxFuture<'_, Result<RelayStream, String>> {
            Box::pin(async move {
                if self.fail {
                    return Err("peer is gone".to_owned());
                }
                let (ours, theirs) = tokio::io::duplex(64 * 1024);
                (self.responder)(theirs);
                let (read, write) = tokio::io::split(ours);
                Ok(RelayStream {
                    send: Box::new(CountedSend {
                        inner: write,
                        resets: Arc::clone(&self.resets),
                    }),
                    recv: Box::new(PlainRecv(read)),
                })
            })
        }
    }
}
