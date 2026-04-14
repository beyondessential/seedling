use std::collections::BTreeSet;

use crate::{
    defs::{container::image_registry, resource::Resource},
    runtime::{db::Db, faults, registries},
};

use super::AppEntry;

const FAULT_KIND: &str = "disallowed_registry";

/// Collect the set of distinct registry hostnames from all container images in
/// the app's evaluated resources.
fn collect_image_registries(entry: &AppEntry) -> BTreeSet<String> {
    let def = entry.app.def.lock();
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

        if let Some(ref img) = image {
            if let Some(reg) = image_registry(img) {
                registries.insert(reg.to_owned());
            }
        }
    }

    registries
}

// l[impl container.image.registry-allowlist]
pub fn sync_registry_faults(db: &Db, entry: &AppEntry) {
    let used = collect_image_registries(entry);

    let allowed: BTreeSet<String> = registries::list_allowed_registries(db)
        .unwrap_or_default()
        .into_iter()
        .collect();

    let disallowed: Vec<&str> = used
        .iter()
        .filter(|r| !allowed.contains(*r))
        .map(String::as_str)
        .collect();

    let existing: Vec<_> = faults::list_active_faults(db, Some(&entry.name))
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.kind == FAULT_KIND)
        .collect();

    if disallowed.is_empty() {
        for f in &existing {
            if let Err(e) = faults::clear_fault(db, &f.id, &entry.name) {
                tracing::warn!(
                    app = %entry.name, fault_id = %f.id,
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
        if let Err(e) = faults::clear_fault(db, &f.id, &entry.name) {
            tracing::warn!(
                app = %entry.name, fault_id = %f.id,
                "failed to clear stale {FAULT_KIND} fault: {e}",
            );
        }
    }

    if let Err(e) = faults::file_fault(db, &entry.name, None, None, None, FAULT_KIND, &description)
    {
        tracing::warn!(app = %entry.name, "failed to file {FAULT_KIND} fault: {e}");
    }
}
