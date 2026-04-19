use std::sync::Arc;

use seedling_protocol::actor::Actor;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const MAX_FIRST_LINE: usize = 1024 * 1024;

// w[transport.webtransport]
// w[wt.actor]
pub async fn proxy_stream(
    mut wt_send: wtransport::SendStream,
    wt_recv: wtransport::RecvStream,
    mut daemon_send: quinn::SendStream,
    mut daemon_recv: quinn::RecvStream,
    actor: Arc<Actor>,
) {
    if let Err(e) = do_proxy(
        &mut wt_send,
        wt_recv,
        &mut daemon_send,
        &mut daemon_recv,
        &actor,
    )
    .await
    {
        tracing::debug!("proxy stream: {e}");
    }
}

async fn do_proxy(
    wt_send: &mut wtransport::SendStream,
    wt_recv: wtransport::RecvStream,
    daemon_send: &mut quinn::SendStream,
    daemon_recv: &mut quinn::RecvStream,
    actor: &Actor,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut wt_buf = BufReader::new(wt_recv);
    let mut first_line = String::new();
    let n = wt_buf.read_line(&mut first_line).await?;
    if n == 0 {
        return Ok(());
    }
    if first_line.len() > MAX_FIRST_LINE {
        return Err(format!("first line too large ({} bytes)", first_line.len()).into());
    }

    let raw = first_line.trim_end_matches(['\n', '\r']);
    let mut json: serde_json::Value = serde_json::from_str(raw)?;
    if let Some(obj) = json.as_object_mut() {
        obj.insert("actor".into(), serde_json::to_value(actor)?);
    }

    let modified = serde_json::to_vec(&json)?;
    daemon_send.write_all(&modified).await?;
    daemon_send.write_all(b"\n").await?;

    // Splice remaining bytes in both directions concurrently.
    // Keep wt_buf (not into_inner) so buffered bytes aren't lost.
    let fwd = async {
        let _ = tokio::io::copy(&mut wt_buf, daemon_send).await;
        let _ = daemon_send.finish();
    };
    let bwd = async {
        let _ = tokio::io::copy(daemon_recv, wt_send).await;
        let _ = wt_send.shutdown().await;
    };
    tokio::join!(fwd, bwd);

    Ok(())
}
