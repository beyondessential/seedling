//! Operator-interface methods for the Canopy relay.
//!
//! Two audiences meet here. `/canopy/offer` and `/canopy/withdraw` are spoken by
//! the client that carries the requests, and are not operator-facing. The rest —
//! status, the setting, a raw request, an immediate report — are operator-facing
//! and each has a CLI wrapper.

use std::sync::Arc;

use seedling_protocol::{
    canopy::Headers,
    error::{ErrorCode, OiError},
    names::OfferId,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{HandlerResult, RequestCtx};
use crate::{
    oi::{
        canopy::{QuicPeer, RelayFailure, WithdrawReason, relay_request},
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

#[derive(Deserialize)]
pub(crate) struct RequestParams {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub headers: Headers,
    #[serde(default)]
    pub body: Option<String>,
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

// i[canopy.request]
// i[canopy.unavailable]
/// Relay one request and return the response, so an operator can exercise the
/// relay without a consumer being wired to it.
pub(crate) async fn request(state: &Arc<OiState>, params: RequestParams) -> HandlerResult {
    if !state.canopy.is_enabled() {
        return Err(OiError::new(
            ErrorCode::CanopyDisabled,
            "Canopy access is turned off for this instance",
        ));
    }
    let offer = state.canopy.current().ok_or_else(|| {
        OiError::new(
            ErrorCode::CanopyUnavailable,
            "no client is currently offering to reach Canopy",
        )
    })?;

    let _slot = state.canopy.acquire_slot().await;
    let body = params.body.unwrap_or_default();
    let response = relay_request(
        &offer,
        &params.method,
        &params.path,
        params.headers,
        body.as_bytes(),
    )
    .await
    .map_err(|e| {
        let code = match e {
            // A client that answered with an error frame, or did not answer at
            // all, is the same thing from an operator's point of view: Canopy
            // was not reached through it.
            RelayFailure::Client(_)
            | RelayFailure::Peer(_)
            | RelayFailure::Timeout
            | RelayFailure::Frame(_) => ErrorCode::CanopyUnavailable,
        };
        OiError::new(code, format!("{e}"))
    })?;

    let mut result = json!({
        "status": response.status,
        "headers": response.headers,
    });
    let map = result.as_object_mut().expect("just built an object");
    // A body that is not text still has to reach the operator somehow, and
    // lossy conversion would show them something Canopy did not send.
    match String::from_utf8(response.body) {
        Ok(text) => {
            map.insert("body".into(), Value::String(text));
        }
        Err(e) => {
            use base64::Engine as _;
            map.insert(
                "body_base64".into(),
                Value::String(base64::engine::general_purpose::STANDARD.encode(e.as_bytes())),
            );
        }
    }
    Ok(result)
}

// i[canopy.report.invoke]
pub(crate) async fn report(state: &Arc<OiState>) -> HandlerResult {
    match crate::runtime::canopy::report(state).await {
        Ok(()) => Ok(json!({ "ok": true })),
        Err(error) => Ok(json!({ "ok": false, "error": error })),
    }
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

    /// A minimal raw-request param set aimed at an endpoint that needs no body.
    fn probe_params() -> RequestParams {
        RequestParams {
            method: "GET".into(),
            path: "/servers/self".into(),
            headers: Headers::new(),
            body: None,
        }
    }

    // i[verify canopy.unavailable]
    #[test]
    fn a_raw_request_with_nothing_offering_says_canopy_is_unavailable() {
        let oi = TestOi::new();
        let err = oi
            .block_on(request(&oi.state, probe_params()))
            .expect_err("nothing to relay through");
        assert!(matches!(err.code, ErrorCode::CanopyUnavailable), "{err:?}");
    }

    // i[verify canopy.unavailable]
    #[test]
    fn a_raw_request_while_disabled_is_refused_as_disabled() {
        let oi = TestOi::new();
        oi.state.canopy.set_enabled(false);
        let err = oi
            .block_on(request(&oi.state, probe_params()))
            .expect_err("disabled");
        assert!(
            matches!(err.code, ErrorCode::CanopyDisabled),
            "an operator who turned it off should not be told the client is missing: {err:?}"
        );
    }

    // i[verify canopy.report.invoke]
    #[test]
    fn an_immediate_report_with_nothing_offering_succeeds_without_reporting() {
        let oi = TestOi::new();
        let result = oi
            .block_on(report(&oi.state))
            .expect("skipping is not an error");
        assert_eq!(
            result["ok"], true,
            "no provider is a deployment choice, not a failure"
        );
        assert_eq!(
            oi.call("/canopy/status", json!({})).unwrap()["last_report"],
            Value::Null,
            "a skipped turn is not an attempt, so it records no outcome"
        );
    }

    // i[verify canopy.report.invoke]
    #[test]
    fn an_immediate_report_while_disabled_also_does_nothing() {
        let oi = TestOi::new();
        oi.state.canopy.set_enabled(false);
        assert_eq!(oi.block_on(report(&oi.state)).unwrap()["ok"], true);
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
