use seedling_protocol::actor::Actor;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader,
};

/// The daemon's own request limit, so a request the daemon would accept,
/// such as a pushed definition bundle, is not refused on the way to it.
const MAX_FIRST_LINE: usize = seedling_protocol::REQUEST_LIMIT;

pub struct PeekedRequest {
    pub method: String,
    /// The first JSON line with the actor field injected, ready to write.
    pub modified_line: Vec<u8>,
    /// Buffered remainder of the WT recv stream.
    pub remaining: BufReader<wtransport::RecvStream>,
}

/// Read one newline-terminated line, refusing to buffer more than `limit`
/// bytes of it.
///
/// A plain `read_line` is unbounded, so checking the length afterwards meant
/// the whole line had already been buffered: the check could report an
/// overrun but not prevent one, and a peer that never sent a newline could
/// make the read allocate without limit. Reading one byte past the limit is
/// enough to tell an over-long line from one that just fits.
async fn read_bounded_line<R>(
    reader: &mut R,
    out: &mut String,
    limit: usize,
) -> std::io::Result<usize>
where
    R: AsyncBufRead + Unpin,
{
    let mut limited = reader.take(limit as u64 + 1);
    limited.read_line(out).await
}

/// Why a request could not be read from its stream.
#[derive(Debug)]
pub enum PeekError {
    /// The request line breaks the rules: it is too long, is not UTF-8, or is
    /// not a JSON object. The client is told, as the daemon tells a client
    /// whose control request it cannot parse.
    Invalid(String),
    /// The stream failed before a request line was read; there is no one to
    /// answer.
    Stream(std::io::Error),
}

impl std::fmt::Display for PeekError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(why) => f.write_str(why),
            Self::Stream(e) => write!(f, "{e}"),
        }
    }
}

/// The error response for a request line that breaks the rules, as one JSON
/// object followed by a newline.
// spec: WEB#request-relay
pub fn invalid_request_response(why: &str) -> Vec<u8> {
    let mut response = serde_json::to_vec(&serde_json::json!({
        "error": { "code": "requirements_invalid", "message": why }
    }))
    .expect("an error response serialises");
    response.push(b'\n');
    response
}

/// Check a request line and set its top-level `actor`, returning the request's
/// method and the line to relay.
// spec: WEB#request-relay
fn prepare_request_line(raw: &str, actor: &Actor) -> Result<(String, Vec<u8>), String> {
    let serde_json::Value::Object(mut members) =
        serde_json::from_str(raw).map_err(|e| format!("the request is not JSON: {e}"))?
    else {
        return Err("the request must be a JSON object".to_owned());
    };
    let method = members
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    members.insert(
        "actor".into(),
        serde_json::to_value(actor).map_err(|e| format!("the actor cannot be written: {e}"))?,
    );
    let line =
        serde_json::to_vec(&members).map_err(|e| format!("the request cannot be written: {e}"))?;
    Ok((method, line))
}

/// Read and parse the first JSON line from a WT recv stream, inject the actor
/// field, and return the method and remaining buffered stream.
// spec: WEB#request-relay
pub async fn peek_request(
    wt_recv: wtransport::RecvStream,
    actor: &Actor,
) -> Result<PeekedRequest, PeekError> {
    let mut buf = BufReader::new(wt_recv);
    let mut first_line = String::new();
    let n = match read_bounded_line(&mut buf, &mut first_line, MAX_FIRST_LINE).await {
        Ok(n) => n,
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            return Err(PeekError::Invalid("the request is not UTF-8".to_owned()));
        }
        Err(e) => return Err(PeekError::Stream(e)),
    };
    if n == 0 {
        return Err(PeekError::Invalid(
            "the stream carried no request".to_owned(),
        ));
    }
    if first_line.len() > MAX_FIRST_LINE {
        return Err(PeekError::Invalid(format!(
            "the request line exceeds {MAX_FIRST_LINE} bytes"
        )));
    }

    let raw = first_line.trim_end_matches(['\n', '\r']);
    let (method, modified_line) = prepare_request_line(raw, actor).map_err(PeekError::Invalid)?;

    Ok(PeekedRequest {
        method,
        modified_line,
        remaining: buf,
    })
}

// spec: WEB#request-relay
// spec: WEB#session-establishment
/// Splice a pre-peeked request through to a daemon stream.
pub async fn proxy_from_peeked(
    wt_send: &mut wtransport::SendStream,
    peeked: PeekedRequest,
    daemon_send: &mut quinn::SendStream,
    daemon_recv: &mut quinn::RecvStream,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    daemon_send.write_all(&peeked.modified_line).await?;
    daemon_send.write_all(b"\n").await?;

    let mut remaining = peeked.remaining;
    let fwd = async {
        let _ = tokio::io::copy(&mut remaining, daemon_send).await;
        let _ = daemon_send.finish();
    };
    let bwd = async {
        let _ = tokio::io::copy(daemon_recv, wt_send).await;
        let _ = wt_send.shutdown().await;
    };
    tokio::join!(fwd, bwd);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Actor, invalid_request_response, prepare_request_line, read_bounded_line};

    // spec: WEB#request-relay
    #[test]
    fn a_request_line_that_is_not_a_json_object_is_refused() {
        let actor = Actor::default();
        for raw in ["not json", "[1]", "42", "\"method\"", "null", ""] {
            assert!(prepare_request_line(raw, &actor).is_err(), "{raw}");
        }
        let (method, line) =
            prepare_request_line(r#"{"method":"/apps/list","actor":{"id":"x"}}"#, &actor).unwrap();
        assert_eq!(method, "/apps/list");
        let relayed: serde_json::Value = serde_json::from_slice(&line).unwrap();
        assert_ne!(
            relayed["actor"]["id"], "x",
            "the client's actor is replaced"
        );
    }

    // spec: WEB#request-relay
    #[test]
    fn the_refusal_is_one_requirements_invalid_object_and_a_newline() {
        let response = invalid_request_response("too long");
        assert_eq!(response.last(), Some(&b'\n'));
        let body: serde_json::Value =
            serde_json::from_slice(&response[..response.len() - 1]).unwrap();
        assert_eq!(body["error"]["code"], "requirements_invalid");
        assert_eq!(body["error"]["message"], "too long");
    }

    // spec: WEB#request-relay
    #[tokio::test]
    async fn a_line_within_the_limit_is_read_whole() {
        let data = b"hello\nrest".to_vec();
        let mut r = tokio::io::BufReader::new(&data[..]);
        let mut out = String::new();
        let n = read_bounded_line(&mut r, &mut out, 64).await.expect("read");
        assert_eq!(n, 6);
        assert_eq!(out, "hello\n");
    }

    // spec: WEB#request-relay
    // The limit has to bound the read, not just describe it afterwards: a peer
    // that never sends a newline could otherwise buffer without limit.
    #[tokio::test]
    async fn an_unterminated_line_stops_at_the_limit() {
        let data = vec![b'a'; 4096];
        let mut r = tokio::io::BufReader::new(&data[..]);
        let mut out = String::new();
        let n = read_bounded_line(&mut r, &mut out, 16).await.expect("read");
        assert_eq!(n, 17, "reads one past the limit, and no further");
        assert_eq!(out.len(), 17);
    }

    // spec: WEB#request-relay
    #[tokio::test]
    async fn an_over_long_line_is_detectable_by_length() {
        let mut data = vec![b'a'; 32];
        data.push(b'\n');
        let mut r = tokio::io::BufReader::new(&data[..]);
        let mut out = String::new();
        read_bounded_line(&mut r, &mut out, 16).await.expect("read");
        assert!(out.len() > 16, "caller can still reject it: {}", out.len());
    }
}
