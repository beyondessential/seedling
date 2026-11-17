use std::collections::BTreeSet;

use seedling_protocol::names::AppName;

use crate::{
    defs::{container::image_registry, resource::Resource},
    runtime::{db::Db, faults, registries},
};

use super::AppEntry;

const FAULT_KIND: &str = "disallowed_registry";

/// Collect the set of distinct registry hostnames from all container images in
/// the app's evaluated resources.
fn collect_image_registries(entry: &AppEntry) -> BTreeSet<String> {
    let def = entry.app.def.load();
    let mut registries = BTreeSet::new();

    for resource in def.resources.values() {
        let image = match resource {
            Resource::Deployment(d) => {
                let dd = d.def.lock();
                let pod = dd.pod.lock();
                let container = pod.container.lock();
                container.image.clone()
            }
            Resource::Job(j) => {
                let jd = j.def.lock();
                let pod = jd.pod.lock();
                let container = pod.container.lock();
                container.image.clone()
            }
            _ => None,
        };

        if let Some(ref img) = image
            && let Some(reg) = image_registry(img)
        {
            registries.insert(reg.to_owned());
        }
    }

    registries
}

// spec: CTR#registry-allowlist
pub fn sync_registry_faults(db: &Db, entry: &AppEntry) {
    sync_used_registries(db, &entry.name, &collect_image_registries(entry));
}

/// File, replace or clear the app's `disallowed_registry` fault for the given
/// registries. A failed read of the allowlist or of the app's faults leaves
/// every fault as it was: neither is an empty list.
// spec: CTR#registry-allowlist
pub fn sync_used_registries(db: &Db, app: &AppName, used: &BTreeSet<String>) {
    let allowed: BTreeSet<String> = match registries::list_allowed_registries(db) {
        Ok(list) => list.into_iter().collect(),
        Err(e) => {
            tracing::warn!(
                app = %app,
                "could not read the registry allowlist, leaving {FAULT_KIND} faults as they are: {e}",
            );
            return;
        }
    };

    let existing: Vec<_> = match faults::list_active_faults(db, Some(app)) {
        Ok(list) => list.into_iter().filter(|f| f.kind == FAULT_KIND).collect(),
        Err(e) => {
            tracing::warn!(
                app = %app,
                "could not read the app's faults, leaving {FAULT_KIND} faults as they are: {e}",
            );
            return;
        }
    };

    let disallowed: Vec<&str> = used
        .iter()
        .filter(|r| !allowed.contains(*r))
        .map(String::as_str)
        .collect();

    if disallowed.is_empty() {
        for f in &existing {
            if let Err(e) = faults::clear_fault(db, &f.id, app) {
                tracing::warn!(
                    app = %app, fault_id = %f.id,
                    "failed to clear {FAULT_KIND} fault: {e}",
                );
            }
        }
        return;
    }

    let description = format!(
        "image references use disallowed registries: {}",
        disallowed.join(", "),
    );

    let already_filed = existing.iter().any(|f| f.description == description);
    if already_filed {
        return;
    }

    for f in &existing {
        if let Err(e) = faults::clear_fault(db, &f.id, app) {
            tracing::warn!(
                app = %app, fault_id = %f.id,
                "failed to clear stale {FAULT_KIND} fault: {e}",
            );
        }
    }

    if let Err(e) = faults::file_fault(db, app, None, None, None, FAULT_KIND, &description) {
        tracing::warn!(app = %app, "failed to file {FAULT_KIND} fault: {e}");
    }
}

#[cfg(test)]
mod tests;
