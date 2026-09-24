//! The two fault kinds a definition's source can raise.

use std::collections::BTreeMap;

use seedling_protocol::names::AppName;
use semver::Version;

use super::Bundle;
use crate::runtime::{
    db::Db,
    faults::{self, FaultKey, FaultMeta, FaultScope},
};

pub const UNSUPPORTED: &str = "definition_unsupported";
pub const SOURCE_MOVED: &str = "definition_source_moved";

/// Hold `definition_unsupported` exactly while `running` does not satisfy the
/// app's definition's requirement. The condition only changes when the
/// definition is replaced or the runtime restarts, so it is converged at
/// those two points.
// r[impl fault.definition-unsupported]
pub fn sync_unsupported(db: &Db, app: &AppName, bundle: &Bundle, running: &Version) {
    let mut current = BTreeMap::new();
    if let Some(req) = bundle.seedling_versions()
        && !req.matches(running)
    {
        let description = format!(
            "the definition requires Seedling {req}, but this is Seedling {running}; it keeps running, unsupported"
        );
        tracing::error!(app = %app, "{description}");
        current.insert(
            FaultKey::app_wide(app, UNSUPPORTED),
            (FaultMeta::default(), description),
        );
    }
    if let Err(e) = faults::sync_faults(
        db,
        &FaultScope::AppKind(app.clone(), UNSUPPORTED.into()),
        &current,
    ) {
        tracing::error!(app = %app, "could not sync {UNSUPPORTED} fault: {e}");
    }
}

/// Clear `definition_source_moved` because the definition was replaced.
// r[impl fault.definition-source-moved]
pub fn clear_source_moved(db: &Db, app: &AppName) {
    sync_source_moved(db, app, None);
}

/// A tag's re-check found it selecting a digest other than the running one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    pub reference: String,
    pub running: String,
    pub selected: String,
}

/// Converge `definition_source_moved` for one app to the outcome of a
/// *successful* re-check: `Some` when the tag moved, `None` when it selects
/// the running digest. A failed re-check must not call this at all, since an
/// unreachable registry shows neither that the tag moved nor that it did not.
// r[impl fault.definition-source-moved]
pub fn sync_source_moved(db: &Db, app: &AppName, moved: Option<Moved>) {
    let mut current = BTreeMap::new();
    if let Some(m) = moved {
        let description = format!(
            "{} now selects {}, but the app runs {}",
            m.reference, m.selected, m.running
        );
        // The subject is the new digest, so a tag moving again replaces the
        // fault rather than leaving it describing a digest it no longer names.
        current.insert(
            FaultKey::new(app, SOURCE_MOVED, m.selected),
            (FaultMeta::default(), description),
        );
    }
    if let Err(e) = faults::sync_faults(
        db,
        &FaultScope::AppKind(app.clone(), SOURCE_MOVED.into()),
        &current,
    ) {
        tracing::error!(app = %app, "could not sync {SOURCE_MOVED} fault: {e}");
    }
}
