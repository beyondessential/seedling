use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use seedling_protocol::client::{ClientAuth, ClientError, OiClient};
use seedling_protocol::keys::ClientIdentity;

pub struct DaemonConn {
    client: OiClient,
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

        let client = OiClient::connect(addr, auth, &identity).await?;
        Ok(Self { client })
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
