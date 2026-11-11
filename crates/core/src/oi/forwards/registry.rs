use std::{collections::HashMap, fmt, net::Ipv6Addr, sync::Arc};

use jiff::Timestamp;
use parking_lot::Mutex;
use seedling_protocol::actor::Actor;
use seedling_protocol::names::{AppName, ForwardId};
use tokio::sync::{mpsc, watch};

// spec: FWD#requesting-a-forward
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardProto {
    Tcp,
    Udp,
}

/// Errors that can occur when managing forwards.
#[derive(Debug)]
pub enum ForwardError {
    KeySpaceExhausted,
}

impl fmt::Display for ForwardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeySpaceExhausted => {
                f.write_str("all 65536 forward keys are in use for this connection")
            }
        }
    }
}

impl std::error::Error for ForwardError {}

// spec: FWD#status-messages
/// A runtime condition to report on a forward's control stream.
pub struct ForwardStatus {
    pub level: &'static str,
    pub message: String,
}

impl ForwardStatus {
    pub fn warn(message: impl Into<String>) -> Self {
        Self {
            level: "warn",
            message: message.into(),
        }
    }
}

/// Count a noun for a message an operator reads: "1 datagram", "3 datagrams".
pub fn plural(count: u64, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// What became of a datagram handed to a UDP forward's relay.
pub enum Delivery {
    Delivered,
    /// The relay is not keeping up; the datagram was dropped and reported.
    Dropped,
    /// The relay has ended.
    RelayGone,
    /// No UDP forward holds the key on this connection.
    UnknownKey,
}

/// A live port-forward entry held in the registry for the lifetime of the forward.
pub struct ForwardEntry {
    pub forward_id: ForwardId,
    pub forward_key: u16,
    /// Stable connection identifier (`quinn::Connection::stable_id()`).
    pub conn_id: usize,
    pub app: AppName,
    pub service: String,
    pub port: u16,
    pub proto: ForwardProto,
    /// Resolved IPv6 address of the target service instance.
    pub target_addr: Ipv6Addr,
    pub opened_at: Timestamp,
    pub actor: Option<Actor>,
    /// Broadcast stop to the control-stream holder and, for UDP forwards, the relay task.
    pub stop_tx: watch::Sender<bool>,
    /// UDP only: channel to deliver incoming QUIC datagram payloads to the relay task.
    pub udp_tx: Option<mpsc::Sender<Vec<u8>>>,
    /// Status messages for the control stream.
    pub status_tx: mpsc::Sender<ForwardStatus>,
    /// Datagrams dropped since the relay last kept up.
    pub udp_dropped: u64,
}

// spec: FWD#listing-forwards
pub struct ForwardRecord {
    pub forward_id: ForwardId,
    pub app: AppName,
    pub service: String,
    pub port: u16,
    pub proto: ForwardProto,
    pub opened_at: Timestamp,
    pub actor: Option<Actor>,
}

// spec: FWD#concurrency
pub struct ForwardRegistry {
    by_id: HashMap<ForwardId, ForwardEntry>,
    /// (conn_id, forward_key) → forward_id for every live forward: the key
    /// allocator's record of what is held, and the hot-path index for UDP
    /// datagram routing.
    conn_key_to_id: HashMap<(usize, u16), ForwardId>,
    /// conn_id → list of forward_ids, for bulk teardown on connection close.
    conn_to_ids: HashMap<usize, Vec<ForwardId>>,
    /// Per-connection forward-key counters.
    key_counters: HashMap<usize, u16>,
}

impl ForwardRegistry {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            by_id: HashMap::new(),
            conn_key_to_id: HashMap::new(),
            conn_to_ids: HashMap::new(),
            key_counters: HashMap::new(),
        }))
    }

    // spec: FWD#forward-keys
    /// Allocate a `forward_key` on the entry's connection and record the entry
    /// under it in the same step, so no other forward can be handed that key
    /// while this one lives. Every forward holds its key, whatever its proto:
    /// removal drops the `(conn_id, forward_key)` mapping for every kind, so a
    /// key only some kinds recorded could be reissued to a UDP forward and
    /// then unmapped when the TCP forward holding it closed.
    pub fn register(
        &mut self,
        conn_id: usize,
        build: impl FnOnce(u16) -> ForwardEntry,
    ) -> Result<u16, ForwardError> {
        let key = self.alloc_key(conn_id)?;
        let entry = build(key);
        debug_assert_eq!((entry.conn_id, entry.forward_key), (conn_id, key));
        let fid = entry.forward_id;
        self.conn_to_ids.entry(conn_id).or_default().push(fid);
        self.conn_key_to_id.insert((conn_id, key), fid);
        self.by_id.insert(fid, entry);
        Ok(key)
    }

    /// Scan from the connection's counter for a key no live forward holds.
    fn alloc_key(&mut self, conn_id: usize) -> Result<u16, ForwardError> {
        let counter = self.key_counters.entry(conn_id).or_insert(0);
        let start = *counter;
        loop {
            let candidate = *counter;
            *counter = counter.wrapping_add(1);
            if !self.conn_key_to_id.contains_key(&(conn_id, candidate)) {
                return Ok(candidate);
            }
            if *counter == start {
                return Err(ForwardError::KeySpaceExhausted);
            }
        }
    }

    /// Remove the entry and return it so the caller can send the stop signal.
    pub fn remove(&mut self, id: &ForwardId) -> Option<ForwardEntry> {
        let entry = self.by_id.remove(id)?;
        self.conn_key_to_id
            .remove(&(entry.conn_id, entry.forward_key));
        if let Some(ids) = self.conn_to_ids.get_mut(&entry.conn_id) {
            ids.retain(|fid| fid != id);
        }
        Some(entry)
    }

    pub fn count(&self) -> usize {
        self.by_id.len()
    }

    // spec: FWD#status-messages
    /// Hand a datagram's payload to the UDP forward holding `key` on the
    /// connection. Datagrams the relay has no room for are dropped, and the
    /// forward is told once when dropping starts and once, with the count,
    /// when a datagram next gets through.
    pub fn deliver_datagram(&mut self, conn_id: usize, key: u16, payload: Vec<u8>) -> Delivery {
        let Some(entry) = self
            .conn_key_to_id
            .get(&(conn_id, key))
            .and_then(|fid| self.by_id.get_mut(fid))
        else {
            return Delivery::UnknownKey;
        };
        let Some(udp_tx) = &entry.udp_tx else {
            return Delivery::UnknownKey;
        };
        match udp_tx.try_send(payload) {
            Ok(()) => {
                // Reserved rather than sent: the count is cleared only once
                // the report is on its way, so a status channel that was full
                // leaves the episode to be reported by a later datagram
                // instead of ending it silently.
                if entry.udp_dropped > 0
                    && let Ok(permit) = entry.status_tx.try_reserve()
                {
                    permit.send(ForwardStatus::warn(format!(
                        "UDP relay caught up after dropping {} to the service",
                        plural(entry.udp_dropped, "datagram")
                    )));
                    entry.udp_dropped = 0;
                }
                Delivery::Delivered
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                if entry.udp_dropped == 0 {
                    let _ = entry.status_tx.try_send(ForwardStatus::warn(
                        "UDP relay is not keeping up; dropping datagrams to the service",
                    ));
                }
                entry.udp_dropped += 1;
                Delivery::Dropped
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Delivery::RelayGone,
        }
    }

    /// Look up where a TCP forward's tunneled connections go, and where to
    /// report a failure to reach it.
    pub fn get_target(
        &self,
        id: &ForwardId,
    ) -> Option<(Ipv6Addr, u16, mpsc::Sender<ForwardStatus>)> {
        self.by_id
            .get(id)
            .map(|e| (e.target_addr, e.port, e.status_tx.clone()))
    }

    // spec: FWD#listing-forwards
    pub fn list(&self, app: Option<&str>) -> Vec<ForwardRecord> {
        self.by_id
            .values()
            .filter(|e| app.is_none_or(|a| e.app == a))
            .map(|e| ForwardRecord {
                forward_id: e.forward_id,
                app: e.app.clone(),
                service: e.service.clone(),
                port: e.port,
                proto: e.proto.clone(),
                opened_at: e.opened_at,
                actor: e.actor.clone(),
            })
            .collect()
    }

    // spec: FWD#forward-lifetime
    /// Remove all forwards for the given connection and return their entries so
    /// the caller can send the stop signal and drop `udp_tx`.
    pub fn remove_by_conn(&mut self, conn_id: usize) -> Vec<ForwardEntry> {
        let ids = self.conn_to_ids.remove(&conn_id).unwrap_or_default();
        self.key_counters.remove(&conn_id);
        ids.into_iter()
            .filter_map(|id| {
                let entry = self.by_id.remove(&id)?;
                self.conn_key_to_id
                    .remove(&(entry.conn_id, entry.forward_key));
                Some(entry)
            })
            .collect()
    }

    // spec: FWD#forward-lifetime
    /// Remove all forwards for `app` where `is_stale` returns `true` and return
    /// their entries so the caller can send stop signals.
    pub fn remove_stale_for_app(
        &mut self,
        app: &str,
        is_stale: impl Fn(&ForwardEntry) -> bool,
    ) -> Vec<ForwardEntry> {
        let stale_ids: Vec<ForwardId> = self
            .by_id
            .values()
            .filter(|e| e.app == app && is_stale(e))
            .map(|e| e.forward_id)
            .collect();
        stale_ids
            .into_iter()
            .filter_map(|id| {
                let entry = self.by_id.remove(&id)?;
                self.conn_key_to_id
                    .remove(&(entry.conn_id, entry.forward_key));
                if let Some(ids) = self.conn_to_ids.get_mut(&entry.conn_id) {
                    ids.retain(|fid| *fid != id);
                }
                Some(entry)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(conn_id: usize, forward_key: u16, proto: ForwardProto) -> ForwardEntry {
        entry_with(conn_id, forward_key, proto, None, mpsc::channel(1).0)
    }

    fn entry_with(
        conn_id: usize,
        forward_key: u16,
        proto: ForwardProto,
        udp_tx: Option<mpsc::Sender<Vec<u8>>>,
        status_tx: mpsc::Sender<ForwardStatus>,
    ) -> ForwardEntry {
        let udp_tx = match proto {
            ForwardProto::Udp => udp_tx.or_else(|| Some(mpsc::channel(1).0)),
            ForwardProto::Tcp => None,
        };
        ForwardEntry {
            forward_id: ForwardId::generate(),
            forward_key,
            conn_id,
            app: AppName::new("app").unwrap(),
            service: "svc".to_owned(),
            port: 53,
            proto,
            target_addr: Ipv6Addr::LOCALHOST,
            opened_at: Timestamp::now(),
            actor: None,
            stop_tx: watch::channel(false).0,
            udp_tx,
            status_tx,
            udp_dropped: 0,
        }
    }

    // spec: FWD#forward-keys
    #[test]
    fn wrapped_key_is_not_reissued_while_a_tcp_forward_holds_it() {
        let registry = ForwardRegistry::new();
        let mut reg = registry.lock();
        let conn = 7;

        let mut tcp_id = None;
        let tcp_key = reg
            .register(conn, |k| {
                let e = entry(conn, k, ForwardProto::Tcp);
                tcp_id = Some(e.forward_id);
                e
            })
            .unwrap();

        // Wrap the counter back round to the TCP forward's key.
        reg.key_counters.insert(conn, tcp_key);
        let (udp_tx, _udp_rx) = mpsc::channel(1);
        let udp_key = reg
            .register(conn, |k| {
                entry_with(conn, k, ForwardProto::Udp, Some(udp_tx), mpsc::channel(1).0)
            })
            .unwrap();
        assert_ne!(udp_key, tcp_key);

        reg.remove(&tcp_id.unwrap()).unwrap();
        assert!(
            matches!(
                reg.deliver_datagram(conn, udp_key, vec![1]),
                Delivery::Delivered
            ),
            "closing the TCP forward must leave the UDP forward routable"
        );
    }

    // spec: FWD#forward-keys
    #[test]
    fn exhausted_key_space_is_refused() {
        let registry = ForwardRegistry::new();
        let mut reg = registry.lock();
        let conn = 1;
        for _ in 0..=u16::MAX {
            reg.register(conn, |k| entry(conn, k, ForwardProto::Tcp))
                .unwrap();
        }
        assert!(matches!(
            reg.register(conn, |k| entry(conn, k, ForwardProto::Udp)),
            Err(ForwardError::KeySpaceExhausted)
        ));
    }

    #[test]
    fn one_of_a_thing_is_not_pluralised() {
        assert_eq!(plural(1, "datagram"), "1 datagram");
        assert_eq!(plural(0, "datagram"), "0 datagrams");
        assert_eq!(plural(2, "datagram"), "2 datagrams");
    }

    // spec: FWD#status-messages
    #[test]
    fn a_caught_up_report_the_status_channel_had_no_room_for_is_not_lost() {
        let registry = ForwardRegistry::new();
        let mut reg = registry.lock();
        let conn = 9;
        let (udp_tx, mut udp_rx) = mpsc::channel(1);
        let (status_tx, mut status_rx) = mpsc::channel(1);
        let key = reg
            .register(conn, |k| {
                entry_with(conn, k, ForwardProto::Udp, Some(udp_tx), status_tx)
            })
            .unwrap();

        // One datagram fills the relay, the next is dropped and takes the
        // status channel's only slot with the report that dropping started.
        reg.deliver_datagram(conn, key, vec![1]);
        reg.deliver_datagram(conn, key, vec![2]);

        // The relay catches up while the status channel is still full, so
        // the episode is not yet over as far as the client knows.
        udp_rx.try_recv().unwrap();
        reg.deliver_datagram(conn, key, vec![3]);

        let started = status_rx.try_recv().unwrap();
        assert!(started.message.contains("dropping"), "{}", started.message);
        assert!(
            status_rx.try_recv().is_err(),
            "the catch-up report had nowhere to go"
        );

        // With room again, the next delivery reports the episode it could not.
        udp_rx.try_recv().unwrap();
        reg.deliver_datagram(conn, key, vec![4]);
        let caught_up = status_rx.try_recv().unwrap();
        assert!(
            caught_up.message.contains("dropping 1 datagram"),
            "{}",
            caught_up.message
        );
    }

    // spec: FWD#status-messages
    #[test]
    fn backpressure_is_reported_once_per_episode() {
        let registry = ForwardRegistry::new();
        let mut reg = registry.lock();
        let conn = 3;
        let (udp_tx, mut udp_rx) = mpsc::channel(1);
        let (status_tx, mut status_rx) = mpsc::channel(16);
        let key = reg
            .register(conn, |k| {
                entry_with(conn, k, ForwardProto::Udp, Some(udp_tx), status_tx)
            })
            .unwrap();

        assert!(matches!(
            reg.deliver_datagram(conn, key, vec![1]),
            Delivery::Delivered
        ));
        for _ in 0..3 {
            assert!(matches!(
                reg.deliver_datagram(conn, key, vec![2]),
                Delivery::Dropped
            ));
        }
        let started = status_rx.try_recv().unwrap();
        assert!(started.message.contains("dropping"), "{}", started.message);
        assert!(status_rx.try_recv().is_err(), "one report per episode");

        udp_rx.try_recv().unwrap();
        assert!(matches!(
            reg.deliver_datagram(conn, key, vec![3]),
            Delivery::Delivered
        ));
        let ended = status_rx.try_recv().unwrap();
        assert!(
            ended.message.contains("dropping 3 datagrams"),
            "{}",
            ended.message
        );

        drop(udp_rx);
        assert!(matches!(
            reg.deliver_datagram(conn, key, vec![4]),
            Delivery::RelayGone
        ));
        assert!(matches!(
            reg.deliver_datagram(conn, key.wrapping_add(1), vec![5]),
            Delivery::UnknownKey
        ));
    }
}
