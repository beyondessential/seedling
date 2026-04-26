use std::{collections::VecDeque, sync::Arc};

use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    sync::{Mutex, broadcast},
};

use crate::actor_activity::ActorActivityRegistry;
use crate::daemon::DaemonConn;

const CACHE_SIZE: usize = 200;
const BROADCAST_CAPACITY: usize = 512;

pub struct EventBroker {
    recent: Mutex<VecDeque<Arc<str>>>,
    tx: broadcast::Sender<Arc<str>>,
    actor_activity: Arc<ActorActivityRegistry>,
}

impl EventBroker {
    pub fn new(actor_activity: Arc<ActorActivityRegistry>) -> Arc<Self> {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Arc::new(Self {
            recent: Mutex::new(VecDeque::with_capacity(CACHE_SIZE)),
            tx,
            actor_activity,
        })
    }

    // w[impl sessions.actor-activity]
    pub async fn publish(&self, line: Arc<str>) {
        self.actor_activity.record_from_event_line(&line);
        let mut recent = self.recent.lock().await;
        if recent.len() >= CACHE_SIZE {
            recent.pop_front();
        }
        recent.push_back(Arc::clone(&line));
        drop(recent);
        let _ = self.tx.send(line);
    }

    /// Serve the cached + live event stream to a connected WT client.
    pub async fn serve_client(self: &Arc<Self>, mut send: wtransport::SendStream) {
        let (cached, mut rx) = {
            let recent = self.recent.lock().await;
            let cached: Vec<Arc<str>> = recent.iter().cloned().collect();
            let rx = self.tx.subscribe();
            (cached, rx)
        };

        for line in cached {
            if send.write_all(line.as_bytes()).await.is_err() {
                return;
            }
            if send.write_all(b"\n").await.is_err() {
                return;
            }
        }

        loop {
            match rx.recv().await {
                Ok(line) => {
                    if send.write_all(line.as_bytes()).await.is_err() {
                        return;
                    }
                    if send.write_all(b"\n").await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "event broker: client lagged, events skipped");
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

/// Runs forever: subscribes to the daemon event stream and forwards to the broker.
/// Reconnects automatically on transport errors.
pub async fn run_event_broker(broker: Arc<EventBroker>, daemon: Arc<DaemonConn>) {
    let mut backoff = std::time::Duration::from_secs(1);
    loop {
        match stream_events(&broker, &daemon).await {
            Ok(()) => {
                tracing::info!("event broker: daemon stream ended, reconnecting");
                backoff = std::time::Duration::from_secs(1);
            }
            Err(e) => {
                tracing::warn!(error = %e, "event broker: stream error, reconnecting");
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
    }
}

async fn stream_events(
    broker: &EventBroker,
    daemon: &DaemonConn,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = daemon.new_events_client().await?;
    let uni = client.subscribe_events().await?;
    let mut buf = BufReader::new(uni);
    let mut line = String::new();
    loop {
        line.clear();
        let n = buf.read_line(&mut line).await?;
        if n == 0 {
            return Ok(());
        }
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if !trimmed.is_empty() {
            broker.publish(Arc::from(trimmed)).await;
        }
    }
}
