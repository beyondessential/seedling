use std::{collections::HashSet, sync::Arc, time::Duration};

use tokio::sync::Mutex;
use tracing::{debug, warn};

use super::{Reconciler, RunningPod};
use crate::{
    defs::resource::Resource,
    runtime::{db::DbHandle, images},
    system::System,
};

/// Run autonomous image GC at most once per this interval. Matches the
/// background GC cadence (see `runtime::gc::GcConfig::interval`).
// r[impl image.gc]
const GC_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Retain images for this long without observed use before autonomous GC
/// is allowed to remove them.
// r[impl image.gc]
const GC_UNUSED_RETAIN: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Per-reconciler scratch state for the image phase. Held in a tokio
/// `Mutex` so it can be updated through an `&Reconciler`.
pub(super) struct ImagePhaseState {
    last_gc: Mutex<Option<std::time::Instant>>,
}

impl ImagePhaseState {
    pub fn new() -> Self {
        Self {
            last_gc: Mutex::new(None),
        }
    }
}

impl Reconciler {
    /// Refresh image bookkeeping and drive warm-image pulls. Called every
    /// tick after `ingest_pod_results`.
    // r[impl actuate.image.warm]
    // r[impl image.pin]
    // r[impl image.track]
    pub(super) async fn reconcile_images(&self, running_pods: impl Iterator<Item = &RunningPod>) {
        let driver = Arc::clone(&self.driver);
        let db = self.db.clone();

        // List everything podman knows about locally.
        let images = match driver.container.list_images().await {
            Ok(list) => list,
            Err(e) => {
                warn!(error = %e, "images: list_images failed, skipping image phase this tick");
                return;
            }
        };

        // Collect (reference, image_id) pairs and a live image_id set.
        let mut ref_pairs: Vec<(String, String)> = Vec::new();
        let mut live_ids: Vec<String> = Vec::with_capacity(images.len());
        for img in &images {
            live_ids.push(img.image_id.clone());
            for reference in img.all_references() {
                ref_pairs.push((reference.to_owned(), img.image_id.clone()));
            }
        }

        // Running containers observed this tick, by image reference and id.
        let mut observed_image_ids: HashSet<String> = HashSet::new();
        let mut observed_refs: HashSet<String> = HashSet::new();
        for pod in running_pods {
            if let Some(image) = image_ref_for_resource(&pod.resource) {
                observed_refs.insert(image);
            }
        }

        // Also inspect containers directly — the image_id is the most
        // reliable link between running state and the pin table, because a
        // reference may resolve to different ids over time as tags move.
        for inst in self.current_running_instances().await {
            if let Ok(Some(state)) = driver.container.inspect(&inst).await
                && let Some(id) = state.image_id
            {
                observed_image_ids.insert(id);
            }
        }

        // Refresh bookkeeping in a single DB call.
        let pairs_for_db = ref_pairs.clone();
        let live_for_db = live_ids.clone();
        let observed_ids_for_db: Vec<String> = observed_image_ids.iter().cloned().collect();
        let observed_refs_for_db: Vec<String> = observed_refs.iter().cloned().collect();

        db.call(move |db| {
            if let Err(e) = images::refresh_references(db, &pairs_for_db) {
                warn!(error = %e, "images: refresh_references failed");
            }
            if let Err(e) = images::prune_tracking_except(db, &live_for_db) {
                warn!(error = %e, "images: prune_tracking_except failed");
            }
            for id in &live_for_db {
                if let Err(e) = images::note_present(db, id) {
                    warn!(image_id = %id, error = %e, "images: note_present failed");
                }
            }
            // Mark-used and pin eviction: touching last_used_at and
            // clearing matching pins whenever we see an image actively used.
            // r[impl image.pin]
            for id in &observed_ids_for_db {
                if let Err(e) = images::mark_used(db, id) {
                    warn!(image_id = %id, error = %e, "images: mark_used failed");
                }
                match images::references_for_image(db, id) {
                    Ok(refs) => {
                        for r in refs {
                            if let Err(e) = images::clear_pins_by_reference(db, &r) {
                                warn!(reference = %r, error = %e, "images: clear_pins_by_reference failed");
                            }
                        }
                    }
                    Err(e) => warn!(image_id = %id, error = %e, "images: references_for_image failed"),
                }
            }
            // Evict pins for references directly observed too, even when we
            // didn't capture an image_id (covers containers still starting).
            for r in &observed_refs_for_db {
                if let Err(e) = images::clear_pins_by_reference(db, r) {
                    warn!(reference = %r, error = %e, "images: clear_pins_by_reference failed");
                }
            }
        });

        // Drive pulls for every outstanding pin.
        // r[impl actuate.image.warm]
        self.drive_warm_pulls().await;

        // Autonomous GC, rate-limited.
        self.maybe_run_image_gc(&driver, &db).await;
    }

    async fn current_running_instances(&self) -> Vec<String> {
        use crate::system::types::ContainerFilter;
        match self
            .driver
            .container
            // Unfiltered: every running container (workload, shell, Caddy,
            // resolver, or operator-started) keeps its image in use and
            // blocks autonomous GC of that image.
            .list(ContainerFilter::default())
            .await
        {
            Ok(list) => list.into_iter().map(|c| c.name).collect(),
            Err(e) => {
                warn!(error = %e, "images: list running containers failed");
                Vec::new()
            }
        }
    }

    async fn drive_warm_pulls(&self) {
        let pins = self
            .db
            .call(|db| images::list_pins(db, None).unwrap_or_default());
        for pin in pins {
            if self.db.call({
                let reference = pin.reference.clone();
                move |db| images::reference_present(db, &reference).unwrap_or(false)
            }) {
                // Already present: nothing to pull. The eviction logic above
                // will retire the pin once a running container is observed.
                continue;
            }
            // Discard the ImageUnavailable result — pulls are async and
            // `ensure_image_available` tracks its own retry/back-off state
            // via the Actuator's `pulling` map.
            let _ = self.actuator.ensure_image_available(&pin.reference).await;
        }
    }

    // r[impl image.gc]
    async fn maybe_run_image_gc(&self, driver: &Arc<System>, db: &DbHandle) {
        let now = std::time::Instant::now();
        {
            let mut guard = self.image_phase_state.last_gc.lock().await;
            match *guard {
                Some(last) if now.duration_since(last) < GC_INTERVAL => return,
                _ => *guard = Some(now),
            }
        }

        let retain_ms = GC_UNUSED_RETAIN.as_millis() as i64;
        let candidates =
            db.call(move |db| images::gc_candidates(db, retain_ms).unwrap_or_default());
        if candidates.is_empty() {
            return;
        }

        // Re-check the ground truth before removing each candidate: it must
        // not be pinned and must not be in use by any running container
        // (workload, shell, infra, or anything else podman is tracking).
        let observed_ids: HashSet<String> = match driver
            .container
            .list(crate::system::types::ContainerFilter::default())
            .await
        {
            Ok(list) => {
                let mut set = HashSet::new();
                for c in list {
                    if let Ok(Some(state)) = driver.container.inspect(&c.name).await
                        && let Some(id) = state.image_id
                    {
                        set.insert(id);
                    }
                }
                set
            }
            Err(e) => {
                warn!(error = %e, "image gc: list running containers failed; skipping this cycle");
                return;
            }
        };

        for row in candidates {
            if observed_ids.contains(&row.image_id) {
                continue;
            }
            let image_id = row.image_id.clone();
            let refs =
                db.call(move |db| images::references_for_image(db, &image_id).unwrap_or_default());
            let pinned = db.call(move |db| {
                images::list_pinned_apps_for_references(
                    db,
                    &refs.iter().map(String::as_str).collect::<Vec<_>>(),
                )
                .unwrap_or_default()
            });
            if !pinned.is_empty() {
                continue;
            }

            match driver.container.remove_image(&row.image_id, false).await {
                Ok(true) => {
                    debug!(image_id = %row.image_id, "image gc: removed unused image");
                    let image_id = row.image_id.clone();
                    db.call(move |db| {
                        let _ = images::drop_tracking(db, &image_id);
                    });
                }
                Ok(false) => {
                    // Already gone; drop the tracking row to stay consistent.
                    let image_id = row.image_id.clone();
                    db.call(move |db| {
                        let _ = images::drop_tracking(db, &image_id);
                    });
                }
                Err(e) => {
                    warn!(image_id = %row.image_id, error = %e, "image gc: remove_image failed");
                }
            }
        }
    }
}

fn image_ref_for_resource(resource: &Resource) -> Option<String> {
    match resource {
        Resource::Deployment(dep) => dep.def.lock().pod.lock().container.lock().image.clone(),
        Resource::Job(job) => job.def.lock().pod.lock().container.lock().image.clone(),
        _ => None,
    }
}
