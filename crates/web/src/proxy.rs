use seedling_protocol::actor::Actor;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

const MAX_FIRST_LINE: usize = 1024 * 1024;

pub struct PeekedRequest {
    pub method: String,
    /// The first JSON line with the actor field injected, ready to write.
    pub modified_line: Vec<u8>,
    /// Buffered remainder of the WT recv stream.
    pub remaining: BufReader<wtransport::RecvStream>,
}

/// Read and parse the first JSON line from a WT recv stream, inject the actor
/// field, and return the method and remaining buffered stream.
pub async fn peek_request(
    wt_recv: wtransport::RecvStream,
    actor: &Actor,
) -> Result<PeekedRequest, Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = BufReader::new(wt_recv);
    let mut first_line = String::new();
    let n = buf.read_line(&mut first_line).await?;
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
