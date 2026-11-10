use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use seedling_protocol::actor::Actor;
use seedling_protocol::client::{ClientAuth, ClientError, OiClient};
use seedling_protocol::keys::ClientIdentity;
use serde_json::json;
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
/// them to registered handlers via the connection's own `UniRouter`.
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

/// One daemon connection and the router for the uni streams it carries.
///
/// QUIC stream IDs are only unique within a connection, so the router lives
/// and dies with its connection.
struct Upstream {
    client: OiClient,
    uni_router: Arc<UniRouter>,
}

impl Upstream {
    fn new(client: OiClient) -> Self {
        let uni_router = Arc::new(UniRouter::new());
        tokio::spawn(run_uni_dispatcher(
            client.connection().clone(),
            Arc::clone(&uni_router),
        ));
        Self { client, uni_router }
    }

    async fn open_bi(&self) -> Result<DaemonBi, ClientError> {
        let (send, recv) = self.client.open_bi().await?;
        Ok(DaemonBi {
            send,
            recv,
            uni_router: Arc::clone(&self.uni_router),
        })
    }
}

/// A bidirectional stream on the shared daemon connection, with the router for
/// uni streams the daemon opens on that same connection.
pub struct DaemonBi {
    pub send: quinn::SendStream,
    pub recv: quinn::RecvStream,
    pub uni_router: Arc<UniRouter>,
}

pub struct DaemonConn {
    inner: tokio::sync::Mutex<Upstream>,
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
            inner: tokio::sync::Mutex::new(Upstream::new(client)),
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
    pub async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ClientError> {
        self.inner.lock().await.client.request(method, params).await
    }

    pub async fn probe(&self) -> Result<(), ClientError> {
        match self
            .inner
            .lock()
            .await
            .client
            .request("/server/ping", json!({}))
            .await
        {
            Ok(_) | Err(ClientError::Api { .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Open a bidirectional stream on the shared daemon connection.
    ///
    /// The returned router belongs to the connection the stream was opened on:
    /// register any uni stream IDs the daemon announces on this stream there.
    pub async fn open_bi(&self) -> Result<DaemonBi, ClientError> {
        // Drop the guard before the match so that try_reconnect can re-acquire
        // the lock. Holding it through the scrutinee would deadlock.
        let result = {
            let upstream = self.inner.lock().await;
            upstream.open_bi().await
        };
        match result {
            Ok(bi) => Ok(bi),
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
        match new_client.request("/server/ping", json!({})).await {
            Ok(_) | Err(ClientError::Api { .. }) => {}
            Err(e) => return Err(e),
        }

        let old = std::mem::replace(&mut *self.inner.lock().await, Upstream::new(new_client));

        // w[impl shells.wire]
        // The old dispatcher holds a handle to its connection, which would keep
        // it open; close it so the dispatcher ends, and release anything still
        // waiting on a stream it can no longer carry.
        old.client
            .connection()
            .close(0u32.into(), b"replaced by reconnect");
        old.uni_router.cancel_all().await;
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
        // The raw variant, not `open_subscription`: the request the gateway
        // relays already carries the browser session's actor, which must not
        // be replaced by the web service's own.
        let log_recv = client.open_subscription_raw(request_bytes).await?;
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
