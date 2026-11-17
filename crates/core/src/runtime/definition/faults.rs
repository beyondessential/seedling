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
pub const RECHECK_FAILED: &str = "definition_recheck_failed";

/// How many re-checks in a row fail before `definition_recheck_failed` is filed.
// spec: RCK#back-off
pub const RECHECK_FAULT_AFTER: u32 = 5;

/// Hold `definition_recheck_failed` against the app, stating the latest
/// failure, once its re-checks have failed `RECHECK_FAULT_AFTER` times in a row.
// spec: FLT#app-faults
pub fn file_recheck_failed(db: &Db, app: &AppName, failures: u32, latest: &str) {
    let key = FaultKey::app_wide(app, RECHECK_FAILED);
    let description = format!(
        "the last {failures} re-checks of the app's definition failed; the latest: {latest}"
    );
    match faults::file_once(db, &key, &FaultMeta::default(), &description) {
        Ok(true) => {}
        Ok(false) => {
            let _ = faults::update_description(db, &key, &description);
        }
        Err(e) => tracing::error!(app = %app, "could not file {RECHECK_FAILED}: {e}"),
    }
}

/// Clear `definition_recheck_failed`: a re-check succeeded, or the definition
/// was replaced.
// spec: FLT#app-faults
pub fn clear_recheck_failed(db: &Db, app: &AppName) {
    if let Err(e) = faults::clear_fault_by_key(db, &FaultKey::app_wide(app, RECHECK_FAILED)) {
        tracing::error!(app = %app, "could not clear {RECHECK_FAILED}: {e}");
    }
}

/// Hold `definition_unsupported` exactly while `running` does not satisfy the
/// app's definition's requirement. The condition only changes when the
/// definition is replaced or the runtime restarts, so it is converged at
/// those two points.
// spec: FLT#app-faults
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
// spec: FLT#app-faults
pub fn clear_source_moved(db: &Db, app: &AppName) {
    sync_source_moved(db, app, None);
    clear_recheck_failed(db, app);
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
// spec: FLT#app-faults
pub fn sync_source_moved(db: &Db, app: &AppName, moved: Option<Moved>) {
    // spec: FLT#app-faults
    // The fault is the app's, with no subject. A tag moving again keeps the
    // fault and brings its description up to the digest it now names.
    let key = FaultKey::app_wide(app, SOURCE_MOVED);
    let mut current = BTreeMap::new();
    let description = moved.map(|m| {
        format!(
            "{} now selects {}, but the app runs {}",
            m.reference, m.selected, m.running
        )
    });
    if let Some(description) = &description {
        current.insert(key.clone(), (FaultMeta::default(), description.clone()));
    }
    let synced = faults::sync_faults(
        db,
        &FaultScope::AppKind(app.clone(), SOURCE_MOVED.into()),
        &current,
    )
    .and_then(|_| match &description {
        Some(description) => faults::update_description(db, &key, description).map(|_| ()),
        None => Ok(()),
    });
    if let Err(e) = synced {
        tracing::error!(app = %app, "could not sync {SOURCE_MOVED} fault: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moved(selected: &str) -> Option<Moved> {
        Some(Moved {
            reference: "registry.example/app:stable".to_owned(),
            running: "sha256:aaaa".to_owned(),
            selected: selected.to_owned(),
        })
    }

    fn active(db: &Db, app: &AppName) -> Vec<(String, String)> {
        faults::list_active_faults(db, Some(app))
            .unwrap()
            .into_iter()
            .filter(|f| f.kind == SOURCE_MOVED)
            .map(|f| (f.subject, f.description))
            .collect()
    }

    // spec: FLT#app-faults
    #[test]
    fn a_moved_source_is_one_app_fault_kept_up_to_the_digest_it_selects() {
        let db = Db::open_in_memory().unwrap();
        let app = AppName::new_unchecked("demo");

        sync_source_moved(&db, &app, moved("sha256:bbbb"));
        sync_source_moved(&db, &app, moved("sha256:cccc"));
        let faults = active(&db, &app);
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert_eq!(faults[0].0, "", "the fault has no subject");
        assert!(faults[0].1.contains("sha256:cccc"), "{faults:?}");

        sync_source_moved(&db, &app, None);
        assert!(active(&db, &app).is_empty());
    }
}
