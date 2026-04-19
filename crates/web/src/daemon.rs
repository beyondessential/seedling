use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use seedling_protocol::actor::Actor;
use seedling_protocol::client::{ClientAuth, ClientError, OiClient};
use seedling_protocol::keys::ClientIdentity;
use serde_json::json;

pub struct DaemonConn {
    inner: tokio::sync::Mutex<OiClient>,
    pub fingerprint: String,
    addr: SocketAddr,
    auth: ClientAuth,
    key_path: PathBuf,
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
        let actor = Self::make_actor(&fingerprint);
        let client = OiClient::connect(addr, auth.clone(), &identity, actor).await?;
        Ok(Self {
            inner: tokio::sync::Mutex::new(client),
            fingerprint,
            addr,
            auth,
            key_path: key_file.to_path_buf(),
        })
    }

    fn make_actor(fingerprint: &str) -> Actor {
        Actor {
            kind: Some("web".to_owned()),
            id: Some(fingerprint[..8].to_owned()),
            display: Some("seedling-web".to_owned()),
            session: None,
        }
    }

    /// Verify that the connection is actually usable.
    ///
    /// In TLS 1.3, client certificate verification happens after the server
    /// sends its Finished message, so `connect` can return Ok even if the
    /// daemon will reject our key. The rejection only surfaces on the first
    /// stream use. A full round-trip request forces the daemon to process
    /// our certificate before we declare the connection healthy.
    pub async fn probe(&self) -> Result<(), ClientError> {
        match self.inner.lock().await.request("ping", json!({})).await {
            Ok(_) | Err(ClientError::Api { .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub async fn open_bi(&self) -> Result<(quinn::SendStream, quinn::RecvStream), ClientError> {
        // Drop the guard before the match so that try_reconnect can re-acquire
        // the lock. Holding it through the scrutinee would deadlock.
        let result = {
            let client = self.inner.lock().await;
            client.open_bi().await
        };
        match result {
            Ok(streams) => Ok(streams),
            Err(ClientError::Transport(_)) => {
                tracing::warn!("daemon transport error — attempting reconnect");
                self.try_reconnect().await?;
                self.inner.lock().await.open_bi().await
            }
            Err(e) => Err(e),
        }
    }

    async fn try_reconnect(&self) -> Result<(), ClientError> {
        let identity = ClientIdentity::load_or_generate(&self.key_path)
            .map_err(|e| ClientError::Connect(Box::new(e)))?
            .0;
        let actor = Self::make_actor(&self.fingerprint);
        let new_client = OiClient::connect(self.addr, self.auth.clone(), &identity, actor).await?;
        match new_client.request("ping", json!({})).await {
            Ok(_) | Err(ClientError::Api { .. }) => {}
            Err(e) => return Err(e),
        }
        *self.inner.lock().await = new_client;
        tracing::info!("daemon reconnected");
        Ok(())
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
