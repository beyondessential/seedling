//! Carrying Canopy requests through a connected client.
//!
//! Seedling has no Canopy identity. A client that does have one may offer to
//! issue Seedling's Canopy requests under its own identity; this module tracks
//! those offers and relays requests over them.
//!
//! The offer is registered by an ordinary control request, so nothing here
//! holds a long-lived stream. Each relayed request instead opens its own
//! server-initiated bidirectional stream, which is what makes concurrent
//! requests independent of one another: QUIC gives each its own flow control
//! and its own cancellation, and the stream's end is the body's end.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use jiff::Timestamp;
use parking_lot::Mutex;
use seedling_protocol::{canopy::MAX_INFLIGHT_RELAYS, names::OfferId};
use tokio::sync::Semaphore;

mod peer;
mod relay;
mod transport;

pub use peer::{QuicPeer, RelayPeer, RelayStream};
pub use relay::{RelayFailure, relay_request};
pub use transport::OiCanopyTransport;

/// A peer that opens no streams, for tests that register an offer to observe the
/// bookkeeping around it rather than to relay anything through it.
#[cfg(test)]
pub(crate) fn test_peer() -> std::sync::Arc<dyn RelayPeer> {
    std::sync::Arc::new(peer::NullPeer)
}

// i[canopy.offer]
/// A live registration by a client offering to carry Canopy requests.
#[derive(Clone)]
pub struct Offer {
    pub offer_id: OfferId,
    /// Stable connection identifier (`quinn::Connection::stable_id()`), so the
    /// offer can be torn down with the connection that made it.
    pub conn_id: usize,
    pub agent: String,
    pub endpoint: String,
    pub via: Option<String>,
    pub offered_at: Timestamp,
    /// How to open a relay stream to the offering client.
    pub peer: std::sync::Arc<dyn RelayPeer>,
    /// Registration order, so the most recent offer can be identified without
    /// depending on `offered_at`, which has no guarantee of being distinct
    /// between two offers registered in the same instant.
    seq: u64,
}

// i[canopy.offer.lifetime]
/// Why an offer ended, reported on the `CanopyWithdrawn` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawReason {
    /// The offering client asked for it.
    Requested,
    /// The connection that made the offer was lost.
    Disconnected,
    /// Canopy access was turned off.
    Disabled,
}

impl WithdrawReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Disconnected => "disconnected",
            Self::Disabled => "disabled",
        }
    }
}

/// Everything the relay needs that outlives a single request.
pub struct CanopyState {
    offers: Mutex<HashMap<OfferId, Offer>>,
    /// conn_id → offer ids, for bulk teardown when a connection closes.
    conn_to_ids: Mutex<HashMap<usize, Vec<OfferId>>>,
    next_seq: AtomicU64,
    // i[canopy.relay.limits]
    /// Bounds how many relayed requests may be in flight at once. Seedling's
    /// own use is one report on a timer; the bound is here to contain a future
    /// consumer rather than to shape today's traffic.
    inflight: Semaphore,
    // r[impl canopy.settings.enabled]
    /// Mirror of the durable setting, so the hot path does not touch the
    /// database. The database remains the source of truth across restarts;
    /// this is loaded from it at startup and updated alongside it.
    enabled: AtomicBool,
    // i[canopy.status]
    /// Outcome of the most recent report attempt, for the status surface. Not
    /// persisted: after a restart there genuinely has been no attempt yet, and
    /// reporting a stale one as current would be worse than reporting none.
    last_report: Mutex<Option<crate::runtime::canopy::LastReport>>,
}

impl Default for CanopyState {
    fn default() -> Self {
        Self::new()
    }
}

impl CanopyState {
    pub fn new() -> Self {
        Self {
            offers: Mutex::new(HashMap::new()),
            conn_to_ids: Mutex::new(HashMap::new()),
            next_seq: AtomicU64::new(0),
            inflight: Semaphore::new(MAX_INFLIGHT_RELAYS),
            enabled: AtomicBool::new(true),
            last_report: Mutex::new(None),
        }
    }

    // i[canopy.status]
    pub fn last_report(&self) -> Option<crate::runtime::canopy::LastReport> {
        self.last_report.lock().clone()
    }

    pub fn set_last_report(&self, report: crate::runtime::canopy::LastReport) {
        *self.last_report.lock() = Some(report);
    }

    // r[impl canopy.settings.enabled]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    // r[impl canopy.settings.enabled]
    /// Update the in-memory mirror of the setting. Persisting it is the
    /// caller's job; this only governs whether new work is admitted.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    // i[canopy.offer]
    /// Register an offer and return its identifier.
    pub fn offer(
        &self,
        conn_id: usize,
        agent: String,
        endpoint: String,
        via: Option<String>,
        peer: std::sync::Arc<dyn RelayPeer>,
    ) -> Offer {
        let offer = Offer {
            offer_id: OfferId::generate(),
            conn_id,
            agent,
            endpoint,
            via,
            offered_at: Timestamp::now(),
            peer,
            seq: self.next_seq.fetch_add(1, Ordering::Relaxed),
        };
        self.conn_to_ids
            .lock()
            .entry(conn_id)
            .or_default()
            .push(offer.offer_id);
        self.offers.lock().insert(offer.offer_id, offer.clone());
        offer
    }

    // i[canopy.withdraw]
    /// End an offer made by `conn_id`. Returns `false` when no such offer
    /// exists, or when it belongs to a different connection — one connection
    /// must not be able to revoke another's offer.
    pub fn withdraw(&self, offer_id: OfferId, conn_id: usize) -> bool {
        let mut offers = self.offers.lock();
        match offers.get(&offer_id) {
            Some(offer) if offer.conn_id == conn_id => {
                offers.remove(&offer_id);
                drop(offers);
                self.unindex(conn_id, offer_id);
                true
            }
            _ => false,
        }
    }

    // i[canopy.offer.lifetime]
    /// Drop every offer made by a connection, for use when it closes.
    pub fn remove_by_conn(&self, conn_id: usize) -> Vec<Offer> {
        let ids = self.conn_to_ids.lock().remove(&conn_id).unwrap_or_default();
        let mut offers = self.offers.lock();
        ids.into_iter()
            .filter_map(|id| offers.remove(&id))
            .collect()
    }

    // i[canopy.offer.disabled]
    /// Drop every offer, for use when Canopy access is turned off.
    ///
    /// Disabling takes effect at once rather than at the offering clients' next
    /// reconnect, so an operator who turns Canopy off sees it stop immediately.
    pub fn revoke_all(&self) -> Vec<Offer> {
        self.conn_to_ids.lock().clear();
        self.offers.lock().drain().map(|(_, o)| o).collect()
    }

    // i[canopy.offer.selection]
    /// The offer that would serve the next request: the most recently
    /// registered one still live.
    ///
    /// Earlier offers stay registered and become eligible again if the newer
    /// one ends, which is what lets a client that reconnects before its old
    /// connection has timed out take over without a gap.
    pub fn current(&self) -> Option<Offer> {
        self.offers.lock().values().max_by_key(|o| o.seq).cloned()
    }

    pub fn get(&self, offer_id: OfferId) -> Option<Offer> {
        self.offers.lock().get(&offer_id).cloned()
    }

    pub fn count(&self) -> usize {
        self.offers.lock().len()
    }

    // i[canopy.relay.limits]
    /// Wait for an in-flight slot. The permit is released when dropped.
    pub async fn acquire_slot(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.inflight
            .acquire()
            .await
            .expect("relay semaphore is never closed")
    }

    fn unindex(&self, conn_id: usize, offer_id: OfferId) {
        let mut index = self.conn_to_ids.lock();
        if let Some(ids) = index.get_mut(&conn_id) {
            ids.retain(|id| *id != offer_id);
            if ids.is_empty() {
                index.remove(&conn_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn state_with_offers(n: usize) -> (CanopyState, Vec<Offer>) {
        let state = CanopyState::new();
        let offers = (0..n)
            .map(|i| {
                state.offer(
                    i,
                    format!("agent-{i}"),
                    "https://example.invalid".into(),
                    None,
                    Arc::new(peer::NullPeer),
                )
            })
            .collect();
        (state, offers)
    }

    // i[verify canopy.offer]
    #[test]
    fn an_offer_is_findable_once_registered() {
        let (state, offers) = state_with_offers(1);
        assert_eq!(state.count(), 1);
        let found = state.get(offers[0].offer_id).expect("registered offer");
        assert_eq!(found.agent, "agent-0");
        assert_eq!(found.endpoint, "https://example.invalid");
    }

    // i[verify canopy.offer.selection]
    #[test]
    fn the_most_recent_offer_serves_the_next_request() {
        let (state, offers) = state_with_offers(3);
        assert_eq!(state.current().unwrap().offer_id, offers[2].offer_id);
    }

    // i[verify canopy.offer.selection]
    #[test]
    fn an_earlier_offer_becomes_eligible_again_when_the_newest_ends() {
        let (state, offers) = state_with_offers(2);
        assert_eq!(state.current().unwrap().offer_id, offers[1].offer_id);

        state.remove_by_conn(offers[1].conn_id);
        assert_eq!(
            state.current().unwrap().offer_id,
            offers[0].offer_id,
            "the older offer takes over rather than the relay going dark"
        );
    }

    // i[verify canopy.withdraw]
    #[test]
    fn a_connection_can_withdraw_its_own_offer() {
        let (state, offers) = state_with_offers(1);
        assert!(state.withdraw(offers[0].offer_id, offers[0].conn_id));
        assert_eq!(state.count(), 0);
        assert!(state.current().is_none());
    }

    // i[verify canopy.withdraw]
    #[test]
    fn a_connection_cannot_withdraw_another_connections_offer() {
        let (state, offers) = state_with_offers(2);
        assert!(
            !state.withdraw(offers[0].offer_id, offers[1].conn_id),
            "conn 1 must not be able to revoke conn 0's offer"
        );
        assert_eq!(state.count(), 2);
    }

    // i[verify canopy.withdraw]
    #[test]
    fn withdrawing_an_unknown_offer_reports_that_rather_than_panicking() {
        let (state, _) = state_with_offers(1);
        assert!(!state.withdraw(OfferId::generate(), 0));
    }

    // i[verify canopy.offer.lifetime]
    #[test]
    fn losing_a_connection_drops_only_its_own_offers() {
        let state = CanopyState::new();
        let a = state.offer(1, "a".into(), "e".into(), None, Arc::new(peer::NullPeer));
        let b = state.offer(1, "b".into(), "e".into(), None, Arc::new(peer::NullPeer));
        let c = state.offer(2, "c".into(), "e".into(), None, Arc::new(peer::NullPeer));

        let dropped = state.remove_by_conn(1);
        let dropped: Vec<_> = dropped.iter().map(|o| o.offer_id).collect();
        assert_eq!(dropped.len(), 2);
        assert!(dropped.contains(&a.offer_id) && dropped.contains(&b.offer_id));
        assert_eq!(state.count(), 1);
        assert_eq!(state.current().unwrap().offer_id, c.offer_id);
    }

    // i[verify canopy.offer.disabled]
    #[test]
    fn disabling_revokes_every_offer_across_connections() {
        let (state, _) = state_with_offers(3);
        let revoked = state.revoke_all();
        assert_eq!(revoked.len(), 3);
        assert_eq!(state.count(), 0);
        assert!(state.current().is_none());
    }

    // i[verify canopy.offer.lifetime]
    #[test]
    fn re_offering_after_teardown_leaves_no_stale_index_entry() {
        let state = CanopyState::new();
        state.offer(7, "a".into(), "e".into(), None, Arc::new(peer::NullPeer));
        state.remove_by_conn(7);
        let again = state.offer(7, "b".into(), "e".into(), None, Arc::new(peer::NullPeer));

        assert_eq!(state.count(), 1);
        assert_eq!(state.current().unwrap().offer_id, again.offer_id);
        assert_eq!(state.remove_by_conn(7).len(), 1, "no phantom ids linger");
    }

    // i[verify canopy.relay.limits]
    #[tokio::test]
    async fn the_in_flight_bound_is_the_declared_one() {
        let state = CanopyState::new();
        let mut held = Vec::new();
        for _ in 0..MAX_INFLIGHT_RELAYS {
            held.push(state.acquire_slot().await);
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), state.acquire_slot())
                .await
                .is_err(),
            "a request past the bound waits rather than opening a stream"
        );
        held.pop();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), state.acquire_slot())
                .await
                .is_ok(),
            "releasing a slot admits the waiter"
        );
    }
}
