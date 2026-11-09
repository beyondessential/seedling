use std::{collections::HashMap, fmt, net::Ipv6Addr, sync::Arc};

use jiff::Timestamp;
use parking_lot::Mutex;
use seedling_protocol::actor::Actor;
use seedling_protocol::names::{AppName, ForwardId};
use tokio::sync::{mpsc, watch};

// i[forward.request]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
}

// i[forward.record]
pub struct ForwardRecord {
    pub forward_id: ForwardId,
    pub app: AppName,
    pub service: String,
    pub port: u16,
    pub proto: ForwardProto,
    pub opened_at: Timestamp,
    pub actor: Option<Actor>,
}

// i[forward.concurrent]
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

    // i[impl forward.key-exhaustion]
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

    /// Look up the UDP payload sender for a `(conn_id, forward_key)` pair.
    pub fn get_udp_sender(&self, conn_id: usize, key: u16) -> Option<&mpsc::Sender<Vec<u8>>> {
        let fid = self.conn_key_to_id.get(&(conn_id, key))?;
        self.by_id.get(fid)?.udp_tx.as_ref()
    }

    /// Look up the target address and port for a forward by id.
    pub fn get_target(&self, id: &ForwardId) -> Option<(Ipv6Addr, u16)> {
        self.by_id.get(id).map(|e| (e.target_addr, e.port))
    }

    // i[forward.list]
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

    // i[forward.lifetime]
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

    // i[forward.script-update]
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
        let udp_tx = matches!(proto, ForwardProto::Udp).then(|| mpsc::channel(1).0);
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
        }
    }

    // i[verify forward.key-exhaustion]
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
        let udp_key = reg
            .register(conn, |k| entry(conn, k, ForwardProto::Udp))
            .unwrap();
        assert_ne!(udp_key, tcp_key);

        reg.remove(&tcp_id.unwrap()).unwrap();
        assert!(
            reg.get_udp_sender(conn, udp_key).is_some(),
            "closing the TCP forward must leave the UDP forward routable"
        );
    }

    // i[verify forward.key-exhaustion]
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
}
