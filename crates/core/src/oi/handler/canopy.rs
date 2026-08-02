//! Operator-interface methods for the Canopy relay.
//!
//! Two audiences meet here. `/canopy/offer` and `/canopy/withdraw` are spoken by
//! the client that carries the requests, and are not operator-facing. The rest —
//! the status and the setting — are operator-facing and each has a CLI wrapper.
//!
//! There is deliberately nothing for relaying a request: the relay carries what
//! the runtime itself needs, and a method for relaying anything else would grant
//! every authorised key the full authority of the carrying client's Canopy
//! identity.

use std::sync::Arc;

use seedling_protocol::{
    error::{ErrorCode, OiError},
    names::OfferId,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{HandlerResult, RequestCtx};
use crate::{
    oi::{
        canopy::{QuicPeer, WithdrawReason},
        state::OiState,
    },
    runtime::canopy::store,
};

#[derive(Deserialize)]
pub(crate) struct OfferParams {
    pub agent: String,
    pub endpoint: String,
    #[serde(default)]
    pub via: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct WithdrawParams {
    pub offer_id: OfferId,
}

#[derive(Deserialize)]
pub(crate) struct SettingsParams {
    pub enabled: bool,
}

// i[canopy.offer]
// i[canopy.offer.disabled]
/// Register the calling connection as a Canopy provider.
///
/// Needs the connection itself, not just the request, so it is dispatched from
/// the stream path rather than from the shared table.
pub(crate) fn offer(
    state: &Arc<OiState>,
    conn: &quinn::Connection,
    params: OfferParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    if !state.canopy.is_enabled() {
        return Err(OiError::new(
            ErrorCode::CanopyDisabled,
            "Canopy access is turned off for this instance; \
             an operator can turn it on with `seedling-ctl canopy enable`",
        ));
    }

    let offer = state.canopy.offer(
        conn.stable_id(),
        params.agent,
        params.endpoint,
        params.via,
        QuicPeer::shared(conn.clone()),
    );
    tracing::info!(
        offer_id = %offer.offer_id,
        agent = %offer.agent,
        endpoint = %offer.endpoint,
        "a client is now carrying Canopy requests"
    );
    ctx.events
        .canopy_offered(offer.offer_id, &offer.agent, &offer.endpoint);
    Ok(json!({ "offer_id": offer.offer_id }))
}

// i[canopy.withdraw]
pub(crate) fn withdraw(
    state: &Arc<OiState>,
    conn: &quinn::Connection,
    params: WithdrawParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    if !state.canopy.withdraw(params.offer_id, conn.stable_id()) {
        return Err(OiError::not_found(format!(
            "no offer {} is registered by this connection",
            params.offer_id
        )));
    }
    tracing::info!(offer_id = %params.offer_id, "a client stopped carrying Canopy requests");
    ctx.events
        .canopy_withdrawn(params.offer_id, WithdrawReason::Requested.as_str());
    // r[impl canopy.report.fault] — if that was the last offer, reporting is no
    // longer expected, so a fault from when it was would misdescribe us.
    crate::runtime::canopy::clear_fault_if_not_expected(state);
    Ok(json!({}))
}

// i[canopy.settings]
pub(crate) fn get_settings(state: &OiState) -> HandlerResult {
    let settings = state
        .db
        .call(store::get_settings)
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db error: {e}")))?;
    Ok(json!({
        "enabled": settings.enabled,
        "updated_at": settings.updated_at,
    }))
}

// i[canopy.settings]
// i[canopy.offer.disabled]
pub(crate) fn set_settings(
    state: &Arc<OiState>,
    params: SettingsParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    let enabled = params.enabled;
    state
        .db
        .call(move |db| store::set_enabled(db, enabled))
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db error: {e}")))?;
    state.canopy.set_enabled(enabled);

    if !enabled {
        // Take effect now rather than at the offering clients' next reconnect,
        // which on a healthy connection may be a very long time away.
        for offer in state.canopy.revoke_all() {
            tracing::info!(
                offer_id = %offer.offer_id,
                "revoking a Canopy offer because access was turned off"
            );
            ctx.events
                .canopy_withdrawn(offer.offer_id, WithdrawReason::Disabled.as_str());
        }
    }
    // r[impl canopy.report.fault] — nothing is expected of a disabled instance.
    crate::runtime::canopy::clear_fault_if_not_expected(state);
    Ok(json!({}))
}

// i[canopy.status]
pub(crate) fn status(state: &Arc<OiState>) -> HandlerResult {
    let settings = state
        .db
        .call(store::get_settings)
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("db error: {e}")))?;

    let offer = state.canopy.current().map(|o| {
        json!({
            "offer_id": o.offer_id,
            "agent": o.agent,
            "endpoint": o.endpoint,
            "via": o.via,
            "offered_at": o.offered_at.to_string(),
        })
    });

    let last_report = state.canopy.last_report().map(|r| {
        let mut value = json!({ "at": r.at.to_string(), "ok": r.ok });
        if let (Some(map), Some(error)) = (value.as_object_mut(), r.error) {
            map.insert("error".into(), Value::String(error));
        }
        value
    });

    Ok(json!({
        "enabled": settings.enabled,
        "offer": offer,
        "offers": state.canopy.count(),
        "server_id": settings.server_id,
        "last_report": last_report,
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::oi::test_support::TestOi;

    // i[verify canopy.status]
    #[test]
    fn a_fresh_instance_is_enabled_with_nothing_offering() {
        let oi = TestOi::new();
        let status = oi.call("/canopy/status", json!({})).unwrap();
        assert_eq!(status["enabled"], true);
        assert_eq!(status["offer"], Value::Null);
        assert_eq!(status["offers"], 0);
        assert_eq!(status["server_id"], Value::Null);
        assert_eq!(
            status["last_report"],
            Value::Null,
            "no attempt has been made yet, and a stale one must not read as current"
        );
    }

    // i[verify canopy.settings]
    #[test]
    fn the_setting_round_trips_through_the_interface() {
        let oi = TestOi::new();
        assert_eq!(
            oi.call("/canopy/settings/get", json!({})).unwrap()["enabled"],
            true
        );

        oi.call("/canopy/settings/set", json!({ "enabled": false }))
            .expect("turning it off");
        assert_eq!(
            oi.call("/canopy/settings/get", json!({})).unwrap()["enabled"],
            false
        );
        assert_eq!(
            oi.call("/canopy/status", json!({})).unwrap()["enabled"],
            false
        );

        oi.call("/canopy/settings/set", json!({ "enabled": true }))
            .expect("turning it back on");
        assert_eq!(
            oi.call("/canopy/settings/get", json!({})).unwrap()["enabled"],
            true
        );
    }

    // i[verify canopy.settings]
    #[test]
    fn setting_it_stamps_when_it_changed() {
        let oi = TestOi::new();
        assert_eq!(
            oi.call("/canopy/settings/get", json!({})).unwrap()["updated_at"],
            0
        );
        oi.call("/canopy/settings/set", json!({ "enabled": false }))
            .unwrap();
        assert!(
            oi.call("/canopy/settings/get", json!({})).unwrap()["updated_at"]
                .as_i64()
                .unwrap()
                > 0
        );
    }

    // i[verify canopy.offer.disabled]
    #[test]
    fn disabling_revokes_a_live_offer_rather_than_waiting_for_a_reconnect() {
        let oi = TestOi::new();
        oi.state.canopy.offer(
            1,
            "test-agent".into(),
            "https://example.invalid".into(),
            None,
            crate::oi::canopy::test_peer(),
        );
        assert_eq!(oi.call("/canopy/status", json!({})).unwrap()["offers"], 1);

        oi.call("/canopy/settings/set", json!({ "enabled": false }))
            .expect("turning it off");

        let status = oi.call("/canopy/status", json!({})).unwrap();
        assert_eq!(status["offers"], 0, "the live offer is gone at once");
        assert_eq!(status["offer"], Value::Null);
    }

    // i[verify canopy.offer.disabled]
    #[test]
    fn re_enabling_does_not_resurrect_a_revoked_offer() {
        let oi = TestOi::new();
        oi.state.canopy.offer(
            1,
            "test-agent".into(),
            "https://example.invalid".into(),
            None,
            crate::oi::canopy::test_peer(),
        );
        oi.call("/canopy/settings/set", json!({ "enabled": false }))
            .unwrap();
        oi.call("/canopy/settings/set", json!({ "enabled": true }))
            .unwrap();

        // The offering client re-offers on its own; nothing here can, because
        // the connection to offer over is the client's to present.
        assert_eq!(oi.call("/canopy/status", json!({})).unwrap()["offers"], 0);
    }

    // i[verify canopy.status]
    #[test]
    fn the_status_describes_the_offer_that_would_serve_the_next_request() {
        let oi = TestOi::new();
        oi.state.canopy.offer(
            1,
            "bestool 9.9.9".into(),
            "https://canopy.invalid".into(),
            Some("mtls".into()),
            crate::oi::canopy::test_peer(),
        );

        let status = oi.call("/canopy/status", json!({})).unwrap();
        assert_eq!(status["offers"], 1);
        assert_eq!(status["offer"]["agent"], "bestool 9.9.9");
        assert_eq!(status["offer"]["endpoint"], "https://canopy.invalid");
        assert_eq!(status["offer"]["via"], "mtls");
        assert!(status["offer"]["offered_at"].is_string());
        assert!(status["offer"]["offer_id"].is_string());
    }

    /// File the report fault, as a failing report would.
    fn file_report_fault(oi: &TestOi) {
        oi.state.db.call(|db| {
            crate::runtime::faults::file_fault(
                db,
                &seedling_protocol::names::AppName::new_unchecked("seedling"),
                None,
                None,
                None,
                "canopy_report_failed",
                "canopy said no",
            )
            .expect("file a fault");
        });
    }

    fn active_faults(oi: &TestOi) -> usize {
        oi.state
            .db
            .call(|db| crate::runtime::faults::list_active_faults(db, None).unwrap_or_default())
            .len()
    }

    // r[verify canopy.report.fault]
    #[test]
    fn disabling_clears_a_report_fault_at_once() {
        // Not at the next scheduled tick: an instance that is no longer expected
        // to report must not be showing a fault for not reporting.
        let oi = TestOi::new();
        oi.state.canopy.offer(
            1,
            "test".into(),
            "https://example.invalid".into(),
            None,
            crate::oi::canopy::test_peer(),
        );
        file_report_fault(&oi);
        assert_eq!(active_faults(&oi), 1);

        oi.call("/canopy/settings/set", json!({ "enabled": false }))
            .expect("turning it off");
        assert_eq!(active_faults(&oi), 0);
    }

    // r[verify canopy.report.fault]
    #[test]
    fn a_report_fault_survives_while_another_offer_still_carries() {
        // The expectation to report has not ended, so neither has the fault.
        let oi = TestOi::new();
        for conn in [1, 2] {
            oi.state.canopy.offer(
                conn,
                "test".into(),
                "https://example.invalid".into(),
                None,
                crate::oi::canopy::test_peer(),
            );
        }
        file_report_fault(&oi);

        let newest = oi.state.canopy.current().expect("an offer");
        assert!(oi.state.canopy.withdraw(newest.offer_id, newest.conn_id));
        crate::runtime::canopy::clear_fault_if_not_expected(&oi.state);
        assert_eq!(
            active_faults(&oi),
            1,
            "one provider remains, so reporting is still expected"
        );

        let last = oi.state.canopy.current().expect("the older offer");
        assert!(oi.state.canopy.withdraw(last.offer_id, last.conn_id));
        crate::runtime::canopy::clear_fault_if_not_expected(&oi.state);
        assert_eq!(active_faults(&oi), 0, "the last one ending ends it");
    }

    // i[verify canopy.withdraw]
    #[test]
    fn withdrawing_is_not_reachable_through_the_operator_table() {
        // /canopy/offer and /canopy/withdraw need the connection they arrived
        // on, so they are dispatched from the stream path. Reaching them through
        // the shared table must fail rather than half-work.
        let oi = TestOi::new();
        let (code, _) = oi
            .call(
                "/canopy/withdraw",
                json!({ "offer_id": OfferId::generate() }),
            )
            .unwrap_err();
        assert_eq!(code, "not_found");
    }
}
