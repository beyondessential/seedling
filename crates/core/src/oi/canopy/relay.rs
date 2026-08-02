//! Issuing one relayed request over one stream.

use seedling_protocol::canopy::{
    FrameError, Headers, MAX_RELAY_BODY, RELAY_TIMEOUT, RelayError, RelayRequest, RelayResponse,
    read_frame, write_frame,
};
use tokio::io::AsyncWriteExt as _;

use super::Offer;

/// A response obtained from Canopy through an offering client.
#[derive(Debug)]
pub struct RelayedResponse {
    pub status: u16,
    pub headers: Headers,
    pub body: Vec<u8>,
}

/// Why a relayed request produced no response.
#[derive(Debug)]
pub enum RelayFailure {
    /// The offering client could not be reached, or its stream broke.
    Peer(String),
    /// The client answered, but reported it obtained nothing from Canopy.
    Client(RelayError),
    /// No complete response arrived within the deadline.
    Timeout,
    /// The client's answer was malformed or past a size limit.
    Frame(FrameError),
}

impl std::fmt::Display for RelayFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Peer(m) => write!(f, "the offering client is unreachable: {m}"),
            Self::Client(e) => write!(f, "the offering client could not reach Canopy: {e}"),
            Self::Timeout => write!(f, "no response within {} seconds", RELAY_TIMEOUT.as_secs()),
            Self::Frame(e) => write!(f, "the offering client's answer was not usable: {e}"),
        }
    }
}

impl std::error::Error for RelayFailure {}

// i[canopy.relay]
/// Relay one request through `offer` and return whatever Canopy answered.
///
/// Any status is a success here, including a non-2xx one: individual endpoints
/// give specific codes meaning, so interpreting them belongs to the caller. Only
/// the absence of a response is an error.
pub async fn relay_request(
    offer: &Offer,
    method: &str,
    path: &str,
    headers: Headers,
    body: &[u8],
) -> Result<RelayedResponse, RelayFailure> {
    let (mut send, recv) = offer.peer.open().await.map_err(RelayFailure::Peer)?;

    let header = RelayRequest {
        canopy: offer.offer_id.to_string(),
        method: method.to_owned(),
        path: path.to_owned(),
        headers,
    };
    write_frame(&mut send, &header, body)
        .await
        .map_err(RelayFailure::Frame)?;
    // Half-close so the client sees the end of the request body. Without this
    // the client would wait for bytes that are never coming.
    send.shutdown()
        .await
        .map_err(|e| RelayFailure::Peer(format!("cannot half-close the relay stream: {e}")))?;
    drop(send);

    // i[canopy.relay.limits]
    // On a timeout the read future is dropped, which drops the stream's read
    // half, which is how the peer is told to stop sending: an answer arriving
    // later has nowhere to go.
    let read = read_frame::<_, RelayResponse>(recv, MAX_RELAY_BODY);
    let (response, body) = tokio::time::timeout(RELAY_TIMEOUT, read)
        .await
        .map_err(|_| RelayFailure::Timeout)?
        .map_err(RelayFailure::Frame)?;

    match response {
        RelayResponse::Response { status, headers } => Ok(RelayedResponse {
            status,
            headers,
            body,
        }),
        // i[canopy.relay.error]
        RelayResponse::Error { error } => Err(RelayFailure::Client(error)),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use seedling_protocol::canopy::RelayErrorCode;
    use tokio::io::{AsyncReadExt as _, DuplexStream};

    use super::super::peer::DuplexPeer;
    use super::super::{CanopyState, Offer};
    use super::*;

    /// Build an offer whose peer hands each opened stream to `respond`.
    fn offer_answering(
        respond: impl Fn(DuplexStream) + Send + Sync + 'static,
    ) -> (CanopyState, Offer) {
        let state = CanopyState::new();
        let offer = state.offer(
            0,
            "test".into(),
            "https://example.invalid".into(),
            None,
            DuplexPeer::new(respond),
        );
        (state, offer)
    }

    /// Read a whole relayed request off the client end of a stream.
    async fn read_request(stream: &mut DuplexStream) -> (RelayRequest, Vec<u8>) {
        let mut all = Vec::new();
        stream.read_to_end(&mut all).await.expect("read request");
        read_frame(&all[..], MAX_RELAY_BODY)
            .await
            .expect("request parses")
    }

    // i[verify canopy.relay]
    #[tokio::test]
    async fn a_request_reaches_the_client_intact_and_its_response_comes_back() {
        let (_state, offer) = offer_answering(|mut stream| {
            tokio::spawn(async move {
                let (req, body) = read_request(&mut stream).await;
                assert_eq!(req.method, "POST");
                assert_eq!(req.path, "/status/abc");
                assert_eq!(req.headers.get("content-encoding").unwrap(), "gzip");
                assert_eq!(body, b"payload");

                write_frame(
                    &mut stream,
                    &RelayResponse::Response {
                        status: 200,
                        headers: Headers::from([(
                            "content-type".to_owned(),
                            "application/json".to_owned(),
                        )]),
                    },
                    br#"{"ok":true}"#,
                )
                .await
                .unwrap();
                stream.shutdown().await.unwrap();
            });
        });

        let headers = Headers::from([("content-encoding".to_owned(), "gzip".to_owned())]);
        let response = relay_request(&offer, "POST", "/status/abc", headers, b"payload")
            .await
            .expect("a response");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, br#"{"ok":true}"#);
        assert_eq!(
            response.headers.get("content-type").unwrap(),
            "application/json"
        );
    }

    // i[verify canopy.relay]
    #[tokio::test]
    async fn the_offer_id_is_carried_so_a_withdrawn_offer_can_be_rejected() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (_state, offer) = offer_answering(move |mut stream| {
            let tx = tx.clone();
            tokio::spawn(async move {
                let (req, _) = read_request(&mut stream).await;
                tx.send(req.canopy).unwrap();
                write_frame(
                    &mut stream,
                    &RelayResponse::Response {
                        status: 204,
                        headers: Headers::new(),
                    },
                    b"",
                )
                .await
                .unwrap();
                stream.shutdown().await.unwrap();
            });
        });

        relay_request(&offer, "GET", "/x", Headers::new(), b"")
            .await
            .expect("a response");
        assert_eq!(rx.recv().unwrap(), offer.offer_id.to_string());
    }

    // i[verify canopy.relay]
    #[tokio::test]
    async fn a_non_success_status_is_a_response_not_a_failure() {
        for status in [403u16, 412, 500] {
            let (_state, offer) = offer_answering(move |mut stream| {
                tokio::spawn(async move {
                    read_request(&mut stream).await;
                    write_frame(
                        &mut stream,
                        &RelayResponse::Response {
                            status,
                            headers: Headers::new(),
                        },
                        b"nope",
                    )
                    .await
                    .unwrap();
                    stream.shutdown().await.unwrap();
                });
            });

            let response = relay_request(&offer, "GET", "/x", Headers::new(), b"")
                .await
                .unwrap_or_else(|e| panic!("{status} should be a response, got {e}"));
            assert_eq!(response.status, status);
            assert_eq!(response.body, b"nope");
        }
    }

    // i[verify canopy.relay.error]
    #[tokio::test]
    async fn an_error_frame_becomes_a_failure_carrying_its_code() {
        let (_state, offer) = offer_answering(|mut stream| {
            tokio::spawn(async move {
                read_request(&mut stream).await;
                write_frame(
                    &mut stream,
                    &RelayResponse::Error {
                        error: RelayError::unreachable("dns lookup failed"),
                    },
                    b"",
                )
                .await
                .unwrap();
                stream.shutdown().await.unwrap();
            });
        });

        let err = relay_request(&offer, "GET", "/x", Headers::new(), b"")
            .await
            .expect_err("an error frame is a failure");
        match err {
            RelayFailure::Client(e) => {
                assert_eq!(e.code, RelayErrorCode::Unreachable);
                assert!(e.message.contains("dns"));
            }
            other => panic!("expected a client error, got {other}"),
        }
    }

    // i[verify canopy.unavailable]
    #[tokio::test]
    async fn a_peer_that_cannot_open_a_stream_fails_immediately() {
        let state = CanopyState::new();
        let offer = state.offer(
            0,
            "test".into(),
            "https://example.invalid".into(),
            None,
            DuplexPeer::failing(),
        );

        let err = relay_request(&offer, "GET", "/x", Headers::new(), b"")
            .await
            .expect_err("no stream, no response");
        assert!(matches!(err, RelayFailure::Peer(_)), "{err}");
    }

    // i[verify canopy.relay.limits]
    #[tokio::test]
    async fn a_client_that_never_answers_times_out() {
        tokio::time::pause();
        let (_state, offer) = offer_answering(|stream| {
            // Hold the stream open and say nothing.
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(86_400)).await;
                drop(stream);
            });
        });

        let call =
            tokio::spawn(
                async move { relay_request(&offer, "GET", "/x", Headers::new(), b"").await },
            );
        tokio::time::advance(RELAY_TIMEOUT + std::time::Duration::from_secs(1)).await;

        let err = call.await.unwrap().expect_err("silence is a timeout");
        assert!(matches!(err, RelayFailure::Timeout), "{err}");
    }

    // i[verify canopy.relay.limits]
    #[tokio::test]
    async fn a_response_body_past_the_ceiling_is_refused() {
        let (_state, offer) = offer_answering(|mut stream| {
            tokio::spawn(async move {
                read_request(&mut stream).await;
                write_frame(
                    &mut stream,
                    &RelayResponse::Response {
                        status: 200,
                        headers: Headers::new(),
                    },
                    &vec![b'x'; MAX_RELAY_BODY + 1],
                )
                .await
                .unwrap();
                stream.shutdown().await.unwrap();
            });
        });

        let err = relay_request(&offer, "GET", "/x", Headers::new(), b"")
            .await
            .expect_err("past the ceiling");
        assert!(
            matches!(err, RelayFailure::Frame(FrameError::TooLarge)),
            "{err}"
        );
    }

    // i[verify stream.canopy]
    #[tokio::test]
    async fn a_client_that_reads_then_closes_without_answering_is_a_truncation() {
        let (_state, offer) = offer_answering(|mut stream| {
            tokio::spawn(async move {
                read_request(&mut stream).await;
                drop(stream);
            });
        });

        let err = relay_request(&offer, "GET", "/x", Headers::new(), b"")
            .await
            .expect_err("an unanswered request is not a 200");
        assert!(
            matches!(err, RelayFailure::Frame(FrameError::Truncated)),
            "{err}"
        );
    }

    // i[verify stream.canopy]
    #[tokio::test]
    async fn a_client_that_vanishes_mid_request_is_a_failure_not_an_empty_response() {
        // The write itself fails here rather than the read, which is a different
        // path to the same requirement: no silence is ever reported as success.
        let (_state, offer) = offer_answering(drop);

        assert!(
            relay_request(&offer, "GET", "/x", Headers::new(), b"")
                .await
                .is_err(),
            "a vanished client must never look like a response"
        );
    }

    /// The peer trait is object-safe and shared, so an offer can be cloned
    /// around without cloning the connection behind it.
    #[tokio::test]
    async fn offers_share_one_peer() {
        let (state, offer) = offer_answering(|_| {});
        let again = state.get(offer.offer_id).expect("registered");
        assert!(Arc::ptr_eq(&offer.peer, &again.peer));
    }
}
