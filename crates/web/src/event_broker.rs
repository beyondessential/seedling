use std::{collections::VecDeque, sync::Arc};

use seedling_protocol::events::OiEvent;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWrite, AsyncWriteExt as _, BufReader},
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
        // w[impl routes.events.replay]
        // Caching and broadcasting under one lock means a subscriber, which
        // snapshots the cache and subscribes under the same lock, sees each
        // event in exactly one of the two, and both in the same order.
        let mut recent = self.recent.lock().await;
        if recent.len() >= CACHE_SIZE {
            recent.pop_front();
        }
        recent.push_back(Arc::clone(&line));
        let _ = self.tx.send(line);
    }

    /// Snapshot the recent events and subscribe to live ones from that point.
    // w[impl routes.events.replay]
    async fn subscribe(&self) -> (Vec<Arc<str>>, broadcast::Receiver<Arc<str>>) {
        let recent = self.recent.lock().await;
        (recent.iter().cloned().collect(), self.tx.subscribe())
    }

    /// Serve the cached + live event stream to a connected WT client.
    pub async fn serve_client(self: &Arc<Self>, mut send: impl AsyncWrite + Unpin) {
        let (cached, mut rx) = self.subscribe().await;

        for line in cached {
            if write_line(&mut send, &line).await.is_err() {
                return;
            }
        }

        loop {
            let line = match rx.recv().await {
                Ok(line) => line,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "event broker: client lagged, events skipped");
                    // w[impl routes.events.gap]
                    gap_marker(Some(n))
                }
                Err(broadcast::error::RecvError::Closed) => return,
            };
            if write_line(&mut send, &line).await.is_err() {
                return;
            }
        }
    }
}

async fn write_line(send: &mut (impl AsyncWrite + Unpin), line: &str) -> std::io::Result<()> {
    send.write_all(line.as_bytes()).await?;
    send.write_all(b"\n").await
}

fn gap_marker(skipped: Option<u64>) -> Arc<str> {
    serde_json::to_string(&OiEvent::events_dropped(skipped))
        .expect("EventsDropped serialises")
        .into()
}

/// Runs forever: subscribes to the daemon event stream and forwards to the broker.
/// Reconnects automatically on transport errors.
pub async fn run_event_broker(broker: Arc<EventBroker>, daemon: Arc<DaemonConn>) {
    let mut backoff = std::time::Duration::from_secs(1);
    let mut resumed = false;
    loop {
        match stream_events(&broker, &daemon, &mut resumed).await {
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

/// Relay one daemon event subscription into the broker. `resumed` records
/// whether an earlier subscription was established, so that events lost
/// between the two are marked as a gap.
async fn stream_events(
    broker: &EventBroker,
    daemon: &DaemonConn,
    resumed: &mut bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = daemon.new_events_client().await?;
    let uni = client.subscribe_events().await?;
    if *resumed {
        // w[impl routes.events.gap]
        broker.publish(gap_marker(None)).await;
    }
    *resumed = true;
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
            // w[impl routes.events.gap]
            // The daemon's own EventsDropped is relayed like any other event.
            broker.publish(Arc::from(trimmed)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn broker() -> Arc<EventBroker> {
        EventBroker::new(Arc::new(ActorActivityRegistry::new()))
    }

    #[tokio::test]
    async fn publish_caches_and_broadcasts() {
        let broker = broker();
        let mut rx = broker.tx.subscribe();
        broker.publish(Arc::from(r#"{"type":"Test"}"#)).await;

        assert_eq!(&*rx.recv().await.unwrap(), r#"{"type":"Test"}"#);
        let recent = broker.recent.lock().await;
        assert_eq!(recent.len(), 1);
        assert_eq!(&*recent[0], r#"{"type":"Test"}"#);
    }

    #[tokio::test]
    async fn cache_evicts_oldest_beyond_capacity() {
        let broker = broker();
        for i in 0..=CACHE_SIZE {
            broker.publish(Arc::from(format!("event-{i}"))).await;
        }
        let recent = broker.recent.lock().await;
        assert_eq!(recent.len(), CACHE_SIZE);
        assert_eq!(&*recent[0], "event-1");
        assert_eq!(&*recent[CACHE_SIZE - 1], format!("event-{CACHE_SIZE}"));
    }

    // w[verify routes.events.replay]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn subscriber_sees_each_event_once_in_order() {
        const N: usize = CACHE_SIZE;
        for _ in 0..50 {
            let broker = broker();
            let publishers: Vec<_> = (0..4)
                .map(|p| {
                    let broker = Arc::clone(&broker);
                    tokio::spawn(async move {
                        for i in 0..N / 4 {
                            broker.publish(Arc::from(format!("{p}-{i}"))).await;
                        }
                    })
                })
                .collect();
            tokio::task::yield_now().await;
            let (cached, mut rx) = broker.subscribe().await;
            for p in publishers {
                p.await.unwrap();
            }

            let mut seen = cached;
            while let Ok(line) = rx.try_recv() {
                seen.push(line);
            }
            let all = broker
                .recent
                .lock()
                .await
                .iter()
                .cloned()
                .collect::<Vec<_>>();
            assert_eq!(seen, all);
        }
    }

    // w[verify routes.events.gap]
    #[tokio::test]
    async fn lagged_client_receives_gap_marker() {
        let broker = broker();
        let (client, mut server) = tokio::io::duplex(64);
        let serving = tokio::spawn({
            let broker = Arc::clone(&broker);
            async move { broker.serve_client(client).await }
        });
        let mut lines = BufReader::new(&mut server).lines();

        broker.publish(Arc::from("ready")).await;
        assert_eq!(lines.next_line().await.unwrap().unwrap(), "ready");

        // The client is not reading, so the broker's write blocks on the
        // full pipe and its receiver falls behind the channel.
        let total = BROADCAST_CAPACITY + 10;
        for i in 0..total {
            broker.publish(Arc::from(format!("event-{i}"))).await;
        }

        let mut gap = None;
        let mut last = String::new();
        while last != format!("event-{}", total - 1) {
            last = lines.next_line().await.unwrap().unwrap();
            if last.contains("EventsDropped") {
                gap = Some(serde_json::from_str::<serde_json::Value>(&last).unwrap());
            }
        }
        let gap = gap.expect("a gap marker before the latest events");
        assert!(gap["skipped"].as_u64().unwrap() > 0);

        serving.abort();
    }

    #[tokio::test]
    async fn publish_without_subscribers_does_not_error() {
        let broker = broker();
        broker.publish(Arc::from("orphan")).await;
        assert_eq!(broker.recent.lock().await.len(), 1);
    }

    // w[verify sessions.actor-activity]
    #[tokio::test]
    async fn publish_records_actor_activity() {
        let registry = Arc::new(ActorActivityRegistry::new());
        let broker = EventBroker::new(Arc::clone(&registry));
        broker
            .publish(Arc::from(
                serde_json::json!({
                    "type": "OperationStarted",
                    "timestamp": jiff::Timestamp::now().to_string(),
                    "app": "demo",
                    "action_name": "install",
                    "actor": { "kind": "web", "id": "felix" },
                })
                .to_string(),
            ))
            .await;
        let entries = registry.list_recent();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["actor_kind"], "web");
        assert_eq!(entries[0]["last_action"], "started install on demo");
    }
}
