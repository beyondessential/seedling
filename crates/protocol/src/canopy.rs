//! Frames for the Canopy relay.
//!
//! Seedling has no Canopy identity of its own. A connected client may offer to
//! carry Seedling's Canopy requests, issuing them under its own identity; this
//! module defines what crosses the wire between the two, so that both sides
//! compile against one definition rather than hand-syncing two.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{
    AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _,
};

/// Header names and values, lower-cased, with repeats combined per RFC 9110.
pub type Headers = BTreeMap<String, String>;

// i[canopy.relay.limits]
/// Ceiling on a relayed body in either direction.
///
/// The wire format is streaming-capable — body bytes run to the end of the
/// stream with no length framing — but both ends currently buffer whole bodies,
/// so a ceiling is needed to bound that buffer.
pub const MAX_RELAY_BODY: usize = 16 * 1024 * 1024;

// i[canopy.relay.limits]
/// How long the server waits for a complete relayed response before resetting
/// the stream.
///
/// Deliberately longer than an offering client's own request timeout, so the
/// client's timeout fires first and reports what actually went wrong instead of
/// the server guessing from a silence.
pub const RELAY_TIMEOUT: Duration = Duration::from_secs(60);

// i[canopy.relay.limits]
/// How many relayed requests may be in flight at once.
pub const MAX_INFLIGHT_RELAYS: usize = 8;

// i[canopy.offer]
/// Params of `/canopy/offer`.
///
/// All three fields are recorded for operator display only; no behaviour
/// depends on their values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfferParams {
    /// The offering program, e.g. `"bestool 0.7.7"`.
    pub agent: String,
    /// The Canopy base URL the offering client will reach.
    pub endpoint: String,
    /// Free-form note on how the client authenticates to Canopy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

// i[canopy.offer]
/// Result of `/canopy/offer`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfferResult {
    pub offer_id: String,
}

// i[canopy.relay]
/// Header line opening a relay stream, server to client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayRequest {
    /// The offer this request is addressed to. A stream that races a withdrawal
    /// is rejected rather than executed against a provider just revoked.
    pub canopy: String,
    pub method: String,
    /// Request target in origin form: path and query, no scheme or authority.
    pub path: String,
    #[serde(default)]
    pub headers: Headers,
}

// i[canopy.relay]
// i[canopy.relay.error]
/// Header line answering a relay stream, client to server.
///
/// Untagged so the two shapes are distinguished by which key is present, as the
/// control protocol does for its own responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RelayResponse {
    /// Canopy answered. Any status is reported this way, including non-2xx:
    /// individual endpoints give specific codes meaning, so interpreting them
    /// is the caller's business and not the relay's.
    Response {
        status: u16,
        #[serde(default)]
        headers: Headers,
    },
    /// No response was obtained from Canopy at all.
    Error { error: RelayError },
}

// i[canopy.relay.error]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayError {
    pub code: RelayErrorCode,
    pub message: String,
}

// i[canopy.relay.error]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayErrorCode {
    /// The `canopy` field named an offer the client does not hold.
    UnknownOffer,
    /// The client could not obtain a response from Canopy.
    Unreachable,
    /// The header was malformed, or described a request the client cannot build.
    InvalidRequest,
}

impl RelayError {
    pub fn new(code: RelayErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn unknown_offer(offer_id: &str) -> Self {
        Self::new(
            RelayErrorCode::UnknownOffer,
            format!("no offer {offer_id} is held by this client"),
        )
    }

    pub fn unreachable(message: impl Into<String>) -> Self {
        Self::new(RelayErrorCode::Unreachable, message)
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(RelayErrorCode::InvalidRequest, message)
    }
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = match self.code {
            RelayErrorCode::UnknownOffer => "unknown_offer",
            RelayErrorCode::Unreachable => "unreachable",
            RelayErrorCode::InvalidRequest => "invalid_request",
        };
        write!(f, "[{code}] {}", self.message)
    }
}

impl std::error::Error for RelayError {}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

/// Something went wrong reading or writing a relay frame.
#[derive(Debug)]
pub enum FrameError {
    Io(std::io::Error),
    /// The header line was not valid JSON of the expected shape.
    Header(serde_json::Error),
    /// The header line ran past what a header could plausibly be, or the body
    /// ran past [`MAX_RELAY_BODY`].
    TooLarge,
    /// The stream ended before a complete header line arrived.
    Truncated,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "relay stream I/O failed: {e}"),
            Self::Header(e) => write!(f, "relay header is not valid: {e}"),
            Self::TooLarge => write!(f, "relay frame exceeds its size limit"),
            Self::Truncated => write!(f, "relay stream ended mid-header"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<std::io::Error> for FrameError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Ceiling on the header line, so a peer that never sends a newline cannot make
/// the reader buffer without bound before the body limit could apply.
const MAX_HEADER_LINE: usize = 64 * 1024;

// i[stream.canopy]
/// Write a header line and body, then half-close.
pub async fn write_frame<W, H>(mut w: W, header: &H, body: &[u8]) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
    H: Serialize,
{
    let mut line = serde_json::to_vec(header).map_err(FrameError::Header)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    w.write_all(body).await?;
    w.flush().await?;
    Ok(())
}

// i[stream.canopy]
/// Read a header line and the body that follows it, to the end of the stream.
///
/// `max_body` bounds the body; a stream carrying more is an error rather than a
/// truncation, so a caller never mistakes a clipped body for a complete one.
pub async fn read_frame<R, H>(r: R, max_body: usize) -> Result<(H, Vec<u8>), FrameError>
where
    R: AsyncRead + Unpin,
    H: serde::de::DeserializeOwned,
{
    let mut reader = tokio::io::BufReader::new(r);

    let mut line = Vec::new();
    let read = (&mut reader)
        .take(MAX_HEADER_LINE as u64)
        .read_until(b'\n', &mut line)
        .await?;
    if read == 0 {
        return Err(FrameError::Truncated);
    }
    if !line.ends_with(b"\n") {
        // Either the stream ended mid-line, or the line ran past the ceiling.
        return Err(if read >= MAX_HEADER_LINE {
            FrameError::TooLarge
        } else {
            FrameError::Truncated
        });
    }
    line.pop();
    let header = serde_json::from_slice(&line).map_err(FrameError::Header)?;

    // Read one byte past the limit so an oversized body is detected rather than
    // silently truncated at exactly the limit.
    let mut body = Vec::new();
    reader
        .take(max_body as u64 + 1)
        .read_to_end(&mut body)
        .await?;
    if body.len() > max_body {
        return Err(FrameError::TooLarge);
    }

    Ok((header, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> Headers {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    // i[verify stream.canopy]
    #[tokio::test]
    async fn request_frame_round_trips_with_a_body() {
        let header = RelayRequest {
            canopy: "c1".into(),
            method: "POST".into(),
            path: "/status/abc".into(),
            headers: headers(&[("content-encoding", "gzip")]),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &header, b"\x1f\x8b body")
            .await
            .unwrap();

        let (back, body): (RelayRequest, Vec<u8>) =
            read_frame(&buf[..], MAX_RELAY_BODY).await.unwrap();
        assert_eq!(back.canopy, "c1");
        assert_eq!(back.method, "POST");
        assert_eq!(back.path, "/status/abc");
        assert_eq!(back.headers.get("content-encoding").unwrap(), "gzip");
        assert_eq!(body, b"\x1f\x8b body");
    }

    // i[verify stream.canopy]
    #[tokio::test]
    async fn frame_round_trips_with_an_empty_body() {
        let header = RelayRequest {
            canopy: "c1".into(),
            method: "GET".into(),
            path: "/servers/self".into(),
            headers: Headers::new(),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &header, b"").await.unwrap();

        let (back, body): (RelayRequest, Vec<u8>) =
            read_frame(&buf[..], MAX_RELAY_BODY).await.unwrap();
        assert_eq!(back.method, "GET");
        assert!(body.is_empty());
    }

    // i[verify canopy.relay]
    #[tokio::test]
    async fn a_response_frame_carries_any_status() {
        for status in [200u16, 403, 412, 500] {
            let mut buf = Vec::new();
            write_frame(
                &mut buf,
                &RelayResponse::Response {
                    status,
                    headers: headers(&[("content-type", "application/json")]),
                },
                b"{}",
            )
            .await
            .unwrap();

            let (back, body): (RelayResponse, Vec<u8>) =
                read_frame(&buf[..], MAX_RELAY_BODY).await.unwrap();
            match back {
                RelayResponse::Response { status: got, .. } => assert_eq!(got, status),
                RelayResponse::Error { error } => panic!("expected a status, got {error}"),
            }
            assert_eq!(body, b"{}");
        }
    }

    // i[verify canopy.relay.error]
    #[tokio::test]
    async fn an_error_frame_is_distinguished_from_a_response() {
        let mut buf = Vec::new();
        write_frame(
            &mut buf,
            &RelayResponse::Error {
                error: RelayError::unreachable("connection refused"),
            },
            b"",
        )
        .await
        .unwrap();

        let (back, body): (RelayResponse, Vec<u8>) =
            read_frame(&buf[..], MAX_RELAY_BODY).await.unwrap();
        match back {
            RelayResponse::Error { error } => {
                assert_eq!(error.code, RelayErrorCode::Unreachable);
                assert!(error.message.contains("refused"));
            }
            RelayResponse::Response { status, .. } => panic!("expected an error, got {status}"),
        }
        assert!(body.is_empty());
    }

    // i[verify canopy.relay.error]
    #[test]
    fn error_codes_use_their_wire_spelling() {
        let json = serde_json::to_value(RelayError::unknown_offer("c9")).unwrap();
        assert_eq!(json["code"], "unknown_offer");
        assert_eq!(
            serde_json::to_value(RelayError::invalid_request("x")).unwrap()["code"],
            "invalid_request"
        );
    }

    // i[verify canopy.relay.limits]
    #[tokio::test]
    async fn a_body_past_the_limit_is_an_error_not_a_truncation() {
        let header = RelayRequest {
            canopy: "c1".into(),
            method: "GET".into(),
            path: "/x".into(),
            headers: Headers::new(),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &header, &[b'x'; 64]).await.unwrap();

        let err = read_frame::<_, RelayRequest>(&buf[..], 32)
            .await
            .expect_err("64 bytes past a 32-byte limit");
        assert!(matches!(err, FrameError::TooLarge), "{err}");
    }

    // i[verify canopy.relay.limits]
    #[tokio::test]
    async fn a_body_exactly_at_the_limit_is_accepted() {
        let header = RelayRequest {
            canopy: "c1".into(),
            method: "GET".into(),
            path: "/x".into(),
            headers: Headers::new(),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &header, &[b'x'; 32]).await.unwrap();

        let (_, body): (RelayRequest, Vec<u8>) = read_frame(&buf[..], 32).await.unwrap();
        assert_eq!(body.len(), 32);
    }

    // i[verify stream.canopy]
    #[tokio::test]
    async fn a_stream_ending_mid_header_is_truncated_not_malformed() {
        let err = read_frame::<_, RelayRequest>(&b"{\"canopy\":\"c1\""[..], MAX_RELAY_BODY)
            .await
            .expect_err("no newline ever arrives");
        assert!(matches!(err, FrameError::Truncated), "{err}");
    }

    // i[verify stream.canopy]
    #[tokio::test]
    async fn an_empty_stream_is_truncated() {
        let err = read_frame::<_, RelayRequest>(&b""[..], MAX_RELAY_BODY)
            .await
            .expect_err("nothing at all");
        assert!(matches!(err, FrameError::Truncated), "{err}");
    }

    // i[verify stream.canopy]
    #[tokio::test]
    async fn a_header_line_past_the_ceiling_is_too_large() {
        let mut line = vec![b'{'; MAX_HEADER_LINE + 16];
        line.push(b'\n');
        let err = read_frame::<_, RelayRequest>(&line[..], MAX_RELAY_BODY)
            .await
            .expect_err("header ceiling");
        assert!(matches!(err, FrameError::TooLarge), "{err}");
    }

    // i[verify stream.canopy]
    #[tokio::test]
    async fn a_malformed_header_is_reported_as_such() {
        let err = read_frame::<_, RelayRequest>(&b"not json\n"[..], MAX_RELAY_BODY)
            .await
            .expect_err("garbage header");
        assert!(matches!(err, FrameError::Header(_)), "{err}");
    }

    // i[verify canopy.offer]
    #[test]
    fn offer_params_omit_an_absent_via() {
        let params = OfferParams {
            agent: "bestool 0.7.7".into(),
            endpoint: "https://example.invalid".into(),
            via: None,
        };
        let json = serde_json::to_value(&params).unwrap();
        assert!(json.get("via").is_none());

        let back: OfferParams = serde_json::from_value(json).unwrap();
        assert_eq!(back.agent, "bestool 0.7.7");
        assert!(back.via.is_none());
    }
}
