//! The fault filed when an app relies on Canopy and no offer is live.

use std::sync::Arc;

use jiff::Timestamp;
use seedling_protocol::names::AppName;

use super::store;
use crate::{
    oi::state::OiState,
    runtime::{
        db::Db,
        faults::{self, FaultKey, FaultMeta, FaultScope},
    },
};

/// Fault kind filed against an app that relies on Canopy while no offer has
/// been live for [`ABSENT_AFTER_SECS`].
pub const FAULT_KIND: &str = "canopy_offer_absent";

// r[impl canopy.offer.absent]
/// How long no offer may have been live before apps relying on Canopy are
/// faulted.
pub const ABSENT_AFTER_SECS: i64 = 15 * 60;

// r[impl canopy.offer.absent]
/// Record the type an app's definition declares, or that it declares none.
///
/// An app that stops relying on Canopy loses its fault with its row.
pub fn sync_app_type(db: &Db, app: &AppName, canopy_type: Option<&str>) -> rusqlite::Result<()> {
    store::sync_app_type(db, app.as_str(), canopy_type)?;
    if canopy_type.is_none() {
        faults::clear_faults_by_kind(db, app, FAULT_KIND)?;
    }
    Ok(())
}

// r[impl canopy.offer.absent]
/// Clear the fault from every app.
pub fn clear_all(db: &Db) -> rusqlite::Result<()> {
    // Through a scope, so the sweep this runs on every tick asks only for this
    // kind rather than reading the host's whole fault list to discard it.
    faults::clear_faults_in_scope(db, &FaultScope::Kind(FAULT_KIND.to_owned()))?;
    Ok(())
}

// r[impl canopy.offer.absent]
/// File the fault against each app whose period has run out.
///
/// This only files. It is retained rather than a condition: at startup the
/// period has not elapsed, since it runs from the daemon's start, so clearing
/// whenever it had not would drop a held fault on every restart. It clears on
/// an offer, on access being disabled, on the app dropping its type and on
/// deregistration, which are handled where they happen. While access is off or
/// an offer is live this clears the fault too, as a sweep that finds either
/// state has been told of the event another way.
pub fn sweep(
    db: &Db,
    now: i64,
    started_at: i64,
    enabled: bool,
    offer_live: bool,
) -> rusqlite::Result<()> {
    if !enabled || offer_live {
        return clear_all(db);
    }

    let settings = store::get_settings(db)?;
    for app in store::list_apps(db)? {
        let since = [
            settings.last_offer_live_at,
            Some(started_at),
            settings.enabled_at,
            Some(app.since),
        ]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(started_at);
        if now < since + ABSENT_AFTER_SECS {
            continue;
        }

        let description = match settings.last_offer_live_at.and_then(timestamp) {
            Some(at) => format!(
                "Canopy is relied on but no client has offered to carry its requests since {at}"
            ),
            None => "Canopy is relied on but no client has ever offered to carry its requests"
                .to_owned(),
        };
        let name = AppName::new_unchecked(&app.app);
        let key = FaultKey::app_wide(&name, FAULT_KIND);
        if faults::file_once(db, &key, &FaultMeta::default(), &description)? {
            tracing::error!(app = %app.app, "{description}");
        }
    }
    Ok(())
}

fn timestamp(secs: i64) -> Option<Timestamp> {
    Timestamp::from_second(secs).ok()
}

/// Record that an offer is live now and run the sweep, as each reporting tick
/// does.
pub(super) fn tick(state: &Arc<OiState>) {
    let now = store::now_secs();
    let offer_live = state.canopy.current().is_some();
    let enabled = state.canopy.is_enabled();
    let started_at = state.started_at.as_second();
    state.db.call(move |db| {
        if offer_live && let Err(e) = store::mark_offer_live(db, now) {
            tracing::warn!("cannot record that a Canopy offer is live: {e}");
        }
        if let Err(e) = sweep(db, now, started_at, enabled, offer_live) {
            tracing::error!("cannot sweep for apps missing a Canopy offer: {e}");
        }
    });
}

/// Record that an offer was live up to now, as when one is made or ends.
pub(crate) fn offer_was_live(state: &OiState) {
    let now = store::now_secs();
    state.db.call(move |db| {
        if let Err(e) = store::mark_offer_live(db, now) {
            tracing::warn!("cannot record that a Canopy offer was live: {e}");
        }
    });
}

#[cfg(test)]
mod tests;
