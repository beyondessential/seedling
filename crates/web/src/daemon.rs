use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use seedling_protocol::actor::Actor;
use seedling_protocol::client::{ClientAuth, ClientError, OiClient};
use seedling_protocol::keys::ClientIdentity;
use serde_json::json;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::sync::{Mutex, oneshot};

/// Routes incoming daemon uni streams to registered handlers by QUIC stream ID.
///
/// When a shell session opens, the daemon opens server-initiated uni streams
/// (stdout, stderr) whose IDs are announced in the handshake JSON. Callers
/// register those IDs here before the streams arrive; the dispatcher delivers
/// them when they come in, parking unknown IDs briefly until a handler
/// registers.
pub struct UniRouter {
    inner: Mutex<UniRouterInner>,
}

struct UniRouterInner {
    /// Callers waiting for a stream by ID.
    waiting: HashMap<u64, oneshot::Sender<quinn::RecvStream>>,
    /// Streams that arrived before their handler registered.
    parked: HashMap<u64, quinn::RecvStream>,
}

impl UniRouter {
    fn new() -> Self {
        Self {
            inner: Mutex::new(UniRouterInner {
                waiting: HashMap::new(),
                parked: HashMap::new(),
            }),
        }
    }

    /// Register interest in a uni stream with the given daemon-side QUIC stream ID.
    ///
    /// If the stream already arrived (parked), the returned receiver resolves
    /// immediately. Otherwise it resolves when the dispatcher delivers it.
    pub async fn register(&self, stream_id: u64) -> oneshot::Receiver<quinn::RecvStream> {
        let (tx, rx) = oneshot::channel();
        let mut inner = self.inner.lock().await;
        if let Some(stream) = inner.parked.remove(&stream_id) {
            let _ = tx.send(stream);
        } else {
            inner.waiting.insert(stream_id, tx);
        }
        rx
    }

    async fn deliver(&self, stream_id: u64, stream: quinn::RecvStream) {
        let mut inner = self.inner.lock().await;
        if let Some(tx) = inner.waiting.remove(&stream_id) {
            let _ = tx.send(stream);
        } else {
            inner.parked.insert(stream_id, stream);
        }
    }

    /// Cancel all outstanding registrations (connection closed/replaced).
    async fn cancel_all(&self) {
        let mut inner = self.inner.lock().await;
        inner.waiting.clear();
        inner.parked.clear();
    }
}

/// Background task that accepts all incoming daemon uni streams and routes
/// them to registered handlers via the `UniRouter`.
async fn run_uni_dispatcher(conn: quinn::Connection, router: Arc<UniRouter>) {
    loop {
        match conn.accept_uni().await {
            Ok(stream) => {
                let stream_id = stream.id().index();
                router.deliver(stream_id, stream).await;
            }
            Err(e) => {
                tracing::debug!("uni dispatcher: connection closed: {e}");
                router.cancel_all().await;
                break;
            }
        }
    }
}

pub struct DaemonConn {
    inner: tokio::sync::Mutex<OiClient>,
    pub fingerprint: String,
    addr: SocketAddr,
    auth: ClientAuth,
    key_path: PathBuf,
    pub uni_router: Arc<UniRouter>,
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

        let uni_router = Arc::new(UniRouter::new());
        let quic_conn = client.connection().clone();
        tokio::spawn(run_uni_dispatcher(quic_conn, Arc::clone(&uni_router)));

        Ok(Self {
            inner: tokio::sync::Mutex::new(client),
            fingerprint,
            addr,
            auth,
            key_path: key_file.to_path_buf(),
            uni_router,
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
    pub async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ClientError> {
        self.inner.lock().await.request(method, params).await
    }

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

    /// Register interest in an incoming daemon uni stream by its QUIC stream ID.
    ///
    /// Call this after parsing the shell handshake response to receive the
    /// daemon's server-initiated stdout/stderr streams.
    pub async fn register_uni(&self, stream_id: u64) -> oneshot::Receiver<quinn::RecvStream> {
        self.uni_router.register(stream_id).await
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

        // Cancel outstanding registrations tied to the old connection.
        self.uni_router.cancel_all().await;

        // Spawn a new dispatcher for the new connection.
        let quic_conn = new_client.connection().clone();
        tokio::spawn(run_uni_dispatcher(quic_conn, Arc::clone(&self.uni_router)));

        *self.inner.lock().await = new_client;
        tracing::info!("daemon reconnected");
        Ok(())
    }

    /// Send a streaming request to the daemon using a fresh, dedicated QUIC
    /// connection and return both the connection handle (to keep it alive) and
    /// the server-initiated unidirectional stream carrying the response payload.
    ///
    /// Using a dedicated connection ensures that dropping the stream (e.g. when
    /// the client disconnects mid-stream) cannot affect the shared connection
    /// used by normal requests.
    pub async fn start_log_stream(
        &self,
        request_bytes: &[u8],
    ) -> Result<(OiClient, quinn::RecvStream), ClientError> {
        let client = self.new_events_client().await?;
        let conn = client.connection().clone();

        let (mut send, recv) = conn
            .open_bi()
            .await
            .map_err(|e| ClientError::Transport(Box::new(e)))?;

        send.write_all(request_bytes)
            .await
            .map_err(|e| ClientError::Transport(Box::new(e)))?;
        send.write_all(b"\n")
            .await
            .map_err(|e| ClientError::Transport(Box::new(e)))?;
        send.finish()
            .map_err(|e| ClientError::Transport(Box::new(e)))?;

        let mut buf = BufReader::new(recv);
        let mut line = String::new();
        buf.read_line(&mut line)
            .await
            .map_err(|e| ClientError::Transport(Box::new(e)))?;

        let log_recv = conn
            .accept_uni()
            .await
            .map_err(|e| ClientError::Transport(Box::new(e)))?;

        Ok((client, log_recv))
    }

    /// Create a fresh, independent `OiClient` for long-running event subscriptions.
    ///
    /// Event streaming holds a stream open indefinitely, so it must not share
    /// the connection managed by `open_bi`.  Each call opens a new QUIC
    /// connection to the daemon.
    pub async fn new_events_client(&self) -> Result<OiClient, ClientError> {
        let identity = ClientIdentity::load_or_generate(&self.key_path)
            .map_err(|e| ClientError::Connect(Box::new(e)))?
            .0;
        let actor = Self::make_actor(&self.fingerprint);
        OiClient::connect(self.addr, self.auth.clone(), &identity, actor).await
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
