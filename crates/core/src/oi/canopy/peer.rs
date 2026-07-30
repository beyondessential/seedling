//! Opening a relay stream to an offering client.
//!
//! In production the peer is the QUIC connection the offer arrived on. The
//! trait exists so the relay can be exercised over an in-memory pipe as well,
//! and so nothing above it has to know that a relay stream is a QUIC stream.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use tokio::io::{AsyncRead, AsyncWrite};

/// The two halves of a relay stream: what the server writes, and what it reads.
///
/// Abandoning a request is expressed by dropping these rather than by a method
/// of its own, because that is what the underlying transport already means by
/// it. Dropping the read half tells the peer to stop sending — the answer is no
/// longer wanted — and dropping the write half after the request has been
/// written is a clean end of the request body. Note that abandoning cannot
/// un-issue a request the peer has already passed to Canopy; it only stops
/// Seedling waiting for an answer it will not use.
pub type RelayStream = (
    Box<dyn AsyncWrite + Send + Unpin>,
    Box<dyn AsyncRead + Send + Unpin>,
);

// i[stream.canopy]
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
    // i[canopy.relay]
    fn open(&self) -> BoxFuture<'_, Result<RelayStream, String>> {
        Box::pin(async move {
            let (send, recv) = self
                .conn
                .open_bi()
                .await
                .map_err(|e| format!("cannot open a relay stream: {e}"))?;
            Ok((
                Box::new(send) as Box<dyn AsyncWrite + Send + Unpin>,
                Box::new(recv) as Box<dyn AsyncRead + Send + Unpin>,
            ))
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
    }

    impl DuplexPeer {
        pub fn new(
            responder: impl Fn(tokio::io::DuplexStream) + Send + Sync + 'static,
        ) -> Arc<Self> {
            Arc::new(Self {
                responder: Box::new(responder),
                fail: false,
            })
        }

        pub fn failing() -> Arc<Self> {
            Arc::new(Self {
                responder: Box::new(|_| {}),
                fail: true,
            })
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
                Ok((
                    Box::new(write) as Box<dyn AsyncWrite + Send + Unpin>,
                    Box::new(read) as Box<dyn AsyncRead + Send + Unpin>,
                ))
            })
        }
    }
}
