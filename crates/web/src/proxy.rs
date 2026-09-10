use seedling_protocol::actor::Actor;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader,
};

const MAX_FIRST_LINE: usize = 1024 * 1024;

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

/// Read and parse the first JSON line from a WT recv stream, inject the actor
/// field, and return the method and remaining buffered stream.
pub async fn peek_request(
    wt_recv: wtransport::RecvStream,
    actor: &Actor,
) -> Result<PeekedRequest, Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = BufReader::new(wt_recv);
    let mut first_line = String::new();
    let n = read_bounded_line(&mut buf, &mut first_line, MAX_FIRST_LINE).await?;
    if n == 0 {
        return Err("empty stream".into());
    }
    if first_line.len() > MAX_FIRST_LINE {
        return Err(format!("first line too large ({} bytes)", first_line.len()).into());
    }

    let raw = first_line.trim_end_matches(['\n', '\r']);
    let mut json: serde_json::Value = serde_json::from_str(raw)?;
    let method = json
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    if let Some(obj) = json.as_object_mut() {
        obj.insert("actor".into(), serde_json::to_value(actor)?);
    }
    let modified_line = serde_json::to_vec(&json)?;

    Ok(PeekedRequest {
        method,
        modified_line,
        remaining: buf,
    })
}

// w[transport.webtransport]
// w[wt.actor]
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
    use super::{MAX_FIRST_LINE, read_bounded_line};

    // w[verify transport.webtransport]
    #[tokio::test]
    async fn a_line_within_the_limit_is_read_whole() {
        let data = b"hello\nrest".to_vec();
        let mut r = tokio::io::BufReader::new(&data[..]);
        let mut out = String::new();
        let n = read_bounded_line(&mut r, &mut out, 64).await.expect("read");
        assert_eq!(n, 6);
        assert_eq!(out, "hello\n");
    }

    // w[verify transport.webtransport]
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

    // w[verify transport.webtransport]
    #[tokio::test]
    async fn an_over_long_line_is_detectable_by_length() {
        let mut data = vec![b'a'; 32];
        data.push(b'\n');
        let mut r = tokio::io::BufReader::new(&data[..]);
        let mut out = String::new();
        read_bounded_line(&mut r, &mut out, 16).await.expect("read");
        assert!(out.len() > 16, "caller can still reject it: {}", out.len());
    }

    // w[verify transport.webtransport]
    #[tokio::test]
    async fn the_configured_limit_is_a_megabyte() {
        assert_eq!(MAX_FIRST_LINE, 1024 * 1024);
    }
}
