use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use seedling_protocol::actor::Actor;
use seedling_protocol::client::{ClientAuth, ClientError, OiClient};
use seedling_protocol::keys::ClientIdentity;

pub struct DaemonConn {
    client: OiClient,
    pub fingerprint: String,
}

impl DaemonConn {
    pub async fn connect(
        addr: SocketAddr,
        auth: ClientAuth,
        key_file: &Path,
    ) -> Result<Self, ClientError> {
        let (identity, is_new) = ClientIdentity::load_or_generate(key_file)
            .map_err(|e| ClientError::Connect(Box::new(e)))?;

        if is_new {
            tracing::info!(
                path = %key_file.display(),
                fingerprint = %identity.fingerprint,
                "generated new daemon client key — authorise this fingerprint in seedlingd"
            );
        } else {
            tracing::info!(
                fingerprint = %identity.fingerprint,
                "loaded daemon client key"
            );
        }

        let fingerprint = identity.fingerprint.clone();
        let actor = Actor {
            kind: Some("web".to_owned()),
            id: Some(identity.fingerprint[..8].to_owned()),
            display: Some("seedling-web".to_owned()),
            session: None,
        };
        let client = OiClient::connect(addr, auth, &identity, actor).await?;
        Ok(Self { client, fingerprint })
    }

    /// Verify that the connection is actually usable.
    ///
    /// In TLS 1.3, client certificate verification happens after the server
    /// sends its Finished message, so `connect` can return Ok even if the
    /// daemon will reject our key. The rejection only surfaces on the first
    /// stream open. Call this immediately after connect to catch it early.
    pub async fn probe(&self) -> Result<(), ClientError> {
        tokio::task::yield_now().await;
        let (mut send, _recv) = self.client.open_bi().await?;
        let _ = send.finish();
        Ok(())
    }

    pub async fn open_bi(&self) -> Result<(quinn::SendStream, quinn::RecvStream), ClientError> {
        self.client.open_bi().await
    }

    /// Default path for the web binary's persistent client key.
    pub fn default_key_path() -> PathBuf {
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("seedling")
            .join("web.key")
    }
}
