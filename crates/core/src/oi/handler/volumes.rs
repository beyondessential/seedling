use seedling_protocol::error::{ErrorCode, HandlerResult, OiError};
use seedling_protocol::names::{
    AppName, AppVolumeName, ExternalVolumeName, HeldVolumeId, SiteVolumeName, VolumeRef,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    oi::{handler::RequestCtx, state::OiState},
    runtime::identity::VolumeName,
    system::volume_store::HeldVolumeMeta,
};

#[cfg(test)]
mod tests;

/// Run an async storage operation from a synchronous handler.
fn run<F: std::future::Future>(operation: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(operation))
}

fn internal(what: &str, e: impl std::fmt::Display) -> OiError {
    OiError::new(ErrorCode::Internal, format!("{what}: {e}"))
}

/// The held volume with this id, `not_found` when there is none.
fn find_held(state: &OiState, id: &HeldVolumeId) -> Result<HeldVolumeMeta, OiError> {
    state
        .driver
        .volume_store
        .held_meta(id)
        .map_err(|e| internal("failed to read the held volume's record", e))?
        .ok_or_else(|| OiError::new(ErrorCode::NotFound, format!("no held volume with id {id}")))
}

// spec: VOLM#listing-held-volumes
pub(crate) fn list_held(state: &OiState) -> HandlerResult {
    let held = state
        .driver
        .volume_store
        .list_held()
        .map_err(|e| internal("failed to list held volumes", e))?;

    let items: Vec<_> = held
        .iter()
        .map(|h| {
            json!({
                "id": h.id,
                "app": h.app,
                "volume_name": h.volume_name,
                "display_name": h.display_name,
                "reason": h.reason,
                "held_at": h.held_at,
            })
        })
        .collect();

    Ok(json!(items))
}

#[derive(Deserialize)]
pub(crate) struct DeleteHeldParams {
    pub id: HeldVolumeId,
}

// spec: VOLM#deleting-a-held-volume
// spec: ACTU#held-volumes
pub(crate) fn delete_held(
    state: &OiState,
    params: DeleteHeldParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    find_held(state, &params.id)?;

    run(state.driver.volume_store.confirm_delete_held(&params.id))
        .map_err(|e| internal("failed to delete held volume", e))?;

    // spec: ACTU#held-volumes
    ctx.events.held_volume_deleted(params.id);

    Ok(json!({ "deleted": true }))
}

#[derive(Deserialize)]
pub(crate) struct RestoreHeldParams {
    pub id: HeldVolumeId,
    /// Optional override for the new managed site volume's name. Defaults
    /// to the held volume's recorded `volume_name` when omitted.
    #[serde(default)]
    pub target_name: Option<SiteVolumeName>,
}

// spec: VOLM#restoring-a-held-volume
// spec: ACTU#held-volumes
pub(crate) fn restore_held(
    state: &OiState,
    params: RestoreHeldParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::site_volumes::{SiteVolumeDef, SiteVolumeKind};

    let held = find_held(state, &params.id)?;

    let target_name = match params.target_name {
        Some(n) => n,
        None => SiteVolumeName::new(&held.volume_name).map_err(|e| {
            OiError::new(
                ErrorCode::RequirementsInvalid,
                format!(
                    "held volume's recorded name {:?} is not a valid site volume name: {e}; pass target_name to override",
                    held.volume_name
                ),
            )
        })?,
    };

    // spec: SITE#site-volume-names
    // spec: ACTU#held-volumes
    let _claim = check_site_name_free(state, &target_name)?;

    // A restore brings a site volume into being with its own event, so it
    // does not also emit a creation event.
    let def = SiteVolumeDef {
        name: target_name.clone(),
        kind: SiteVolumeKind::Managed,
        created_at: jiff::Timestamp::now().to_string(),
    };
    record_site_volume_then(
        state,
        def,
        || {
            run(state
                .driver
                .volume_store
                .restore_held_to_site(&params.id, target_name.as_str()))
            .map(|_| ())
        },
        "restore held volume",
    )?;

    // spec: ACTU#held-volumes
    ctx.events
        .held_volume_restored(params.id, target_name.as_str());

    Ok(json!({ "restored": true, "name": target_name }))
}

/// Record a new site volume and take its storage, leaving neither if the
/// storage cannot be taken.
///
/// The record goes first, as a held volume's does: a failure after it removes
/// the record again, where a failure after storage was made would leave
/// storage with no record, which a later request would refuse the name over.
// spec: VOLM#requests
fn record_site_volume_then(
    state: &OiState,
    def: crate::runtime::site_volumes::SiteVolumeDef,
    storage: impl FnOnce() -> std::io::Result<()>,
    what: &str,
) -> Result<(), OiError> {
    let name = def.name.clone();
    state
        .db
        .call(move |db| crate::runtime::site_volumes::create(db, &def))
        .map_err(|e| internal(&format!("failed to record the site volume for {what}"), e))?;
    if let Err(e) = storage() {
        let unrecord = name.clone();
        if let Err(undo) = state
            .db
            .call(move |db| crate::runtime::site_volumes::delete(db, &unrecord))
        {
            tracing::error!(
                site_volume = %name,
                error = %undo,
                "could not remove the record of a site volume whose storage failed"
            );
        }
        return Err(internal(&format!("failed to {what}"), e));
    }
    Ok(())
}

pub(crate) fn list_exported(state: &OiState) -> HandlerResult {
    let registry = state.registry.read();
    let mut exported = Vec::new();

    for app_name in registry.names() {
        let Some(entry) = registry.get(app_name.as_str()) else {
            continue;
        };
        let def = entry.app.def.load();
        for (id, resource) in &def.resources {
            if let crate::defs::resource::Resource::Volume(vol) = resource {
                let vol_def = vol.def.lock();
                if let Some(export_opts) = &vol_def.exported {
                    // spec: ACTU#exported-volumes
                    // An exported volume exists to be shared, and a co-located
                    // tool that wants to use its contents (a socket directory,
                    // say) needs somewhere to look. Report where it lives so a
                    // consumer doesn't have to reconstruct the layout.
                    let host_path = state
                        .driver
                        .volume_store
                        .path(&VolumeName::for_app(app_name.as_str(), id.name.as_str()));
                    let mut entry = json!({
                        "app": app_name,
                        "volume_name": id.name.as_str(),
                        "host_path": host_path,
                    });
                    if let Some(desc) = &export_opts.description {
                        entry["description"] = json!(desc);
                    }
                    exported.push(entry);
                }
            }
        }
    }

    sort_by_app_then_volume(&mut exported);
    Ok(json!(exported))
}

/// Order records by `app`, then `volume_name`, each in ascending byte order.
fn sort_by_app_then_volume(records: &mut [serde_json::Value]) {
    let key = |v: &serde_json::Value| {
        (
            v["app"].as_str().unwrap_or_default().to_owned(),
            v["volume_name"].as_str().unwrap_or_default().to_owned(),
        )
    };
    records.sort_by_key(key);
}

/// Every named, persistent (non-tmpfs) app volume, whether or not it is
/// exported. Used by the multi-volume shell picker so operators can pull
/// internal-only volumes into a recovery session alongside exported ones.
pub(crate) fn list_app_volumes(state: &OiState) -> HandlerResult {
    let registry = state.registry.read();
    let mut volumes = Vec::new();

    for app_name in registry.names() {
        let Some(entry) = registry.get(app_name.as_str()) else {
            continue;
        };
        let def = entry.app.def.load();
        for (id, resource) in &def.resources {
            if let crate::defs::resource::Resource::Volume(vol) = resource {
                let vol_def = vol.def.lock();
                if vol_def.tmpfs {
                    continue;
                }
                let mut item = json!({
                    "app": app_name,
                    "volume_name": id.name.as_str(),
                    "exported": vol_def.exported.is_some(),
                });
                if let Some(desc) = vol_def
                    .exported
                    .as_ref()
                    .and_then(|e| e.description.as_ref())
                {
                    item["description"] = json!(desc);
                }
                volumes.push(item);
            }
        }
    }

    sort_by_app_then_volume(&mut volumes);
    Ok(json!(volumes))
}

#[derive(Deserialize)]
pub(crate) struct CreateSiteVolumeParams {
    pub name: SiteVolumeName,
    /// "managed" or "bind"
    pub kind: String,
    /// Required when kind is "bind"
    pub host_path: Option<String>,
}

// spec: VOLM#creating-a-site-volume
// spec: SITE#site-volumes
pub(crate) fn create_site_volume(
    state: &OiState,
    params: CreateSiteVolumeParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::site_volumes::{SiteVolumeDef, SiteVolumeKind};

    let kind = match params.kind.as_str() {
        "managed" => {
            if params.host_path.is_some() {
                return Err(OiError::new(
                    ErrorCode::RequirementsInvalid,
                    "host_path is only for bind site volumes".to_string(),
                ));
            }
            SiteVolumeKind::Managed
        }
        "bind" => {
            let host_path = params.host_path.ok_or_else(|| {
                OiError::new(
                    ErrorCode::RequirementsInvalid,
                    "host_path is required for bind site volumes".to_string(),
                )
            })?;
            if !std::path::Path::new(&host_path).is_absolute() {
                return Err(OiError::new(
                    ErrorCode::RequirementsInvalid,
                    "host_path must be an absolute path".to_string(),
                ));
            }
            SiteVolumeKind::Bind { host_path }
        }
        other => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!("invalid site volume kind: {other:?}, expected \"managed\" or \"bind\""),
            ));
        }
    };

    // spec: SITE#site-volume-names
    let _claim = check_site_name_free(state, &params.name)?;

    let def = SiteVolumeDef {
        name: params.name.clone(),
        kind: kind.clone(),
        created_at: jiff::Timestamp::now().to_string(),
    };

    // For managed volumes, create the backing storage.
    record_site_volume_then(
        state,
        def,
        || {
            if kind == SiteVolumeKind::Managed {
                run(state.driver.volume_store.create_site(params.name.as_str()))
            } else {
                Ok(())
            }
        },
        "create site volume storage",
    )?;

    // spec: SITE#site-volumes
    let (kind_str, host_path) = match &kind {
        SiteVolumeKind::Managed => ("managed", None),
        SiteVolumeKind::Bind { host_path } => ("bind", Some(host_path.as_str())),
        SiteVolumeKind::Snapshot { .. } => unreachable!("create_site_volume never builds Snapshot"),
    };
    ctx.events
        .site_volume_created(params.name.as_str(), kind_str, host_path);

    Ok(json!({ "created": true }))
}

pub(crate) fn list_site_volumes(state: &OiState) -> HandlerResult {
    use crate::runtime::site_volumes::SiteVolumeKind;

    let volumes = state
        .db
        .call(crate::runtime::site_volumes::list)
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to list site volumes: {e}"),
            )
        })?;

    let items: Vec<_> = volumes
        .iter()
        .map(|v| {
            let mut obj = json!({
                "name": v.name,
                "kind": match &v.kind {
                    SiteVolumeKind::Managed => "managed",
                    SiteVolumeKind::Bind { .. } => "bind",
                    SiteVolumeKind::Snapshot { .. } => "snapshot",
                },
                "created_at": v.created_at,
            });
            if let SiteVolumeKind::Bind { host_path } = &v.kind {
                obj["host_path"] = json!(host_path);
            }
            if let SiteVolumeKind::Snapshot { source } = &v.kind {
                obj["source"] = json!(volume_ref_display(source));
            }
            obj
        })
        .collect();

    Ok(json!(items))
}

#[derive(Deserialize)]
pub(crate) struct DeleteSiteVolumeParams {
    pub name: SiteVolumeName,
}

// spec: VOLM#deleting-a-site-volume
// spec: SITE#site-volumes
pub(crate) fn delete_site_volume(
    state: &OiState,
    params: DeleteSiteVolumeParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::site_volumes::{self, Deletion, SiteVolumeKind};

    // The record goes first, in the same step as the look for a mapping that
    // targets it, so none is left targeting a deleted volume. A storage
    // failure after it puts the record back.
    let name_owned = params.name.clone();
    let def = match state
        .db
        .call(move |db| site_volumes::delete_unmapped(db, &name_owned))
        .map_err(|e| internal("failed to delete site volume", e))?
    {
        Deletion::Deleted(def) => def,
        Deletion::Absent => {
            return Err(OiError::new(
                ErrorCode::NotFound,
                format!("no site volume named {:?}", params.name.as_str()),
            ));
        }
        Deletion::Mapped { app, external_name } => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!(
                    "site volume {:?} is the target of the mapping of external volume {:?} \
                     of app {:?}; unmap it first",
                    params.name.as_str(),
                    external_name.as_str(),
                    app.as_str()
                ),
            ));
        }
    };

    // spec: ACTU#held-volumes
    // spec: SITE#site-volumes
    // Managed kinds carry runtime-owned data and route through the held-volume
    // mechanism so an operator must explicitly confirm the final removal.
    // Snapshot kinds are deleted directly: snapshots are read-only BTRFS
    // subvolumes (so the rename used by the hold path returns EROFS), and
    // their source volume remains intact so there is nothing to recover from
    // a held copy. Bind kinds only reference an operator-provided host path,
    // so dropping the row is sufficient.
    //
    // Storage that is not there is held by doing nothing, so the record goes
    // and the operator is not left with a site volume they cannot remove. A
    // hold that could not be carried out still stops the delete: there the
    // data is real, and dropping the record would orphan it.
    let storage = match &def.kind {
        SiteVolumeKind::Managed => run(state
            .driver
            .volume_store
            .hold_site(params.name.as_str(), "site volume deleted by operator"))
        .map_err(|e| internal("failed to hold site volume for review", e)),
        SiteVolumeKind::Snapshot { .. } => {
            run(state.driver.volume_store.remove_site(params.name.as_str()))
                .map(|()| None)
                .map_err(|e| internal("failed to remove site snapshot", e))
        }
        SiteVolumeKind::Bind { .. } => Ok(None),
    };
    let held_meta = match storage {
        Ok(held) => held,
        Err(e) => {
            let restore = def.clone();
            if let Err(undo) = state.db.call(move |db| site_volumes::create(db, &restore)) {
                tracing::error!(
                    site_volume = %params.name,
                    error = %undo,
                    "could not put back the record of a site volume whose storage was not removed"
                );
            }
            return Err(e);
        }
    };

    let held_id = held_meta.as_ref().map(|m| m.id);
    let kind_str = match def.kind {
        SiteVolumeKind::Managed => "managed",
        SiteVolumeKind::Bind { .. } => "bind",
        SiteVolumeKind::Snapshot { .. } => "snapshot",
    };
    if let Some(meta) = &held_meta {
        // spec: ACTU#held-volumes
        //
        // Site-originated holds carry the synthetic "_site" marker in place
        // of an app name. Bypass validation so the pseudo-name round-trips
        // through the event.
        let held_app = AppName::new_unchecked(meta.app.clone());
        ctx.events
            .held_volume_created(meta.id, &held_app, &meta.volume_name, &meta.reason);
    }
    // spec: SITE#site-volumes
    ctx.events
        .site_volume_deleted(params.name.as_str(), kind_str, held_id);

    Ok(json!({ "deleted": true }))
}

/// Refuse a name another site volume already holds, before any storage
/// operation runs against it.
///
/// Both halves matter. A recorded name would only collide when the record is
/// written, which for a snapshot is after btrfs has already placed the
/// subvolume. Storage with no record is refused rather than reused because
/// the runtime cannot tell a leftover from an operator's volume whose record
/// has yet to be written.
///
/// The returned guard is the claim on the name. Hold it until the name has
/// been taken — until both the storage and the record are in place — so that
/// no other request can find the name free in the meantime.
// spec: SITE#site-volume-names
fn check_site_name_free<'a>(
    state: &'a OiState,
    name: &SiteVolumeName,
) -> Result<parking_lot::MutexGuard<'a, ()>, OiError> {
    // spec: SITE#site-volume-names
    // The claim is returned rather than dropped here, and the caller holds
    // it until the name has actually been taken. Finding the name free is
    // only worth anything if nothing else can take it in between: two
    // requests naming the same target would otherwise both pass this and
    // both go on to make storage.
    let claim = state.driver.volume_store.claim_site_name();

    let name_owned = name.clone();
    let recorded = state
        .db
        .call(move |db| crate::runtime::site_volumes::get(db, &name_owned))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to look up site volume: {e}"),
            )
        })?;

    if recorded.is_some() {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("a site volume named {:?} already exists", name.as_str()),
        ));
    }

    if state.driver.volume_store.site_exists(name.as_str()) {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "storage for a site volume named {:?} is already on disk with no site volume \
                 recorded for it; resolve it by hand before reusing the name",
                name.as_str()
            ),
        ));
    }

    Ok(claim)
}

#[derive(Deserialize)]
pub(crate) struct SnapshotSiteVolumeParams {
    /// Name for the new snapshot site volume
    pub name: SiteVolumeName,
    /// Source volume ID: _site/<name> for a managed site volume, or <app>/<volume> for an app volume
    pub source: String,
}

// spec: VOLM#snapshotting-a-volume
// spec: SITE#site-volume-snapshots
pub(crate) fn snapshot_site_volume(
    state: &OiState,
    params: SnapshotSiteVolumeParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::site_volumes::{SiteVolumeDef, SiteVolumeKind};

    let (source, source_path) = parse_source_vol_id(&params.source, state)?;

    // spec: SITE#site-volume-names
    let _claim = check_site_name_free(state, &params.name)?;

    resolve_snapshot_source(state, &source, &source_path)?;

    let event_source = source.clone();
    let def = SiteVolumeDef {
        name: params.name.clone(),
        kind: SiteVolumeKind::Snapshot { source },
        created_at: jiff::Timestamp::now().to_string(),
    };
    record_site_volume_then(
        state,
        def,
        || {
            run(state
                .driver
                .volume_store
                .snapshot_site(params.name.as_str(), &source_path))
        },
        "create snapshot",
    )?;

    // spec: SITE#site-volume-snapshots
    ctx.events
        .site_volume_snapshotted(params.name.as_str(), &event_source);

    Ok(json!({ "created": true, "name": params.name }))
}

/// Check that a snapshot's source can be snapshotted: it has storage, which
/// is a BTRFS subvolume, and is not itself a bind or snapshot site volume.
// spec: VOLM#snapshotting-a-volume
fn resolve_snapshot_source(
    state: &OiState,
    source: &VolumeRef,
    source_path: &std::path::Path,
) -> Result<(), OiError> {
    use crate::runtime::site_volumes::SiteVolumeKind;

    if let VolumeRef::Site { name } = source {
        let recorded = name.clone();
        let def = state
            .db
            .call(move |db| crate::runtime::site_volumes::get(db, &recorded))
            .map_err(|e| internal("failed to look up the source site volume", e))?
            .ok_or_else(|| {
                OiError::new(
                    ErrorCode::NotFound,
                    format!("no site volume named {:?}", name.as_str()),
                )
            })?;
        match def.kind {
            SiteVolumeKind::Managed => {}
            SiteVolumeKind::Bind { .. } | SiteVolumeKind::Snapshot { .. } => {
                return Err(OiError::new(
                    ErrorCode::RequirementsInvalid,
                    format!(
                        "site volume {:?} is not a managed site volume; only an app volume or a \
                         managed site volume can be snapshotted",
                        name.as_str()
                    ),
                ));
            }
        }
    }
    match std::fs::symlink_metadata(source_path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(OiError::new(
                ErrorCode::NotFound,
                format!("{} has no storage to snapshot", volume_ref_display(source)),
            ));
        }
        Err(e) => return Err(internal("failed to look at the source's storage", e)),
    }
    require_btrfs_subvolume(state, source_path)
}

/// `requirements_invalid` when the volume at `path` is not stored as a BTRFS
/// subvolume, wherever the data directory is.
// spec: SITE#site-volume-snapshots
fn require_btrfs_subvolume(state: &OiState, path: &std::path::Path) -> Result<(), OiError> {
    if !run(state.driver.volume_store.is_subvolume(path)) {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            "snapshotting needs a BTRFS subvolume, and this volume is not stored as one",
        ));
    }
    Ok(())
}

/// `requirements_invalid` on a host whose data directory is not on BTRFS.
// spec: SITE#snapshot-promotion
fn require_btrfs_data_dir(state: &OiState) -> Result<(), OiError> {
    if !state.driver.volume_store.uses_btrfs() {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            "promotion needs the data directory to be on BTRFS, and it is not",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct PromoteSiteVolumeParams {
    /// Name of the existing snapshot site volume to promote
    pub source: SiteVolumeName,
    /// Name for the new managed site volume
    pub name: SiteVolumeName,
}

// spec: VOLM#promoting-a-snapshot
// spec: SITE#snapshot-promotion
pub(crate) fn promote_site_volume(
    state: &OiState,
    params: PromoteSiteVolumeParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::site_volumes::{SiteVolumeDef, SiteVolumeKind};

    let source_name = params.source.clone();
    let source_def = state
        .db
        .call(move |db| crate::runtime::site_volumes::get(db, &source_name))
        .map_err(|e| internal("failed to look up source site volume", e))?
        .ok_or_else(|| {
            OiError::new(
                ErrorCode::NotFound,
                format!("no site volume named {:?}", params.source.as_str()),
            )
        })?;

    if !matches!(source_def.kind, SiteVolumeKind::Snapshot { .. }) {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "site volume {:?} is not a snapshot; only snapshot site volumes can be promoted",
                params.source.as_str()
            ),
        ));
    }
    // spec: SITE#site-volume-names
    let _claim = check_site_name_free(state, &params.name)?;

    require_btrfs_data_dir(state)?;

    let source_path = state.driver.volume_store.site_path(params.source.as_str());
    let def = SiteVolumeDef {
        name: params.name.clone(),
        kind: SiteVolumeKind::Managed,
        created_at: jiff::Timestamp::now().to_string(),
    };
    record_site_volume_then(
        state,
        def,
        || {
            run(state
                .driver
                .volume_store
                .promote_site_snapshot(params.name.as_str(), &source_path))
        },
        "promote snapshot",
    )?;

    // spec: SITE#snapshot-promotion
    ctx.events
        .site_volume_promoted(params.name.as_str(), params.source.as_str());

    Ok(json!({ "promoted": true, "name": params.name }))
}

/// Resolve a volume id like `_site/<name>` or `<app>/<volume>` into a
/// `(VolumeRef, on-disk path)` pair. For app-scoped volumes the on-disk path
/// is pulled from the registry's display_name rather than rebuilt from the
/// BSL name — the naming scheme ("<app>-volume-<name>" for kind=Volume via
/// the default arm of new_singleton) is not something any caller should
/// hand-roll.
///
/// A source that is not a volume string is `requirements_invalid`; an app
/// volume that has never had an instance is `not_found`, and a history that
/// cannot be read is `internal`.
fn parse_source_vol_id(
    vol_id: &str,
    state: &OiState,
) -> Result<(VolumeRef, std::path::PathBuf), OiError> {
    let invalid = |why: String| OiError::new(ErrorCode::RequirementsInvalid, why);
    let (prefix, vol) = vol_id.split_once('/').ok_or_else(|| {
        invalid(format!(
            "invalid source {vol_id:?}: expected _site/<name> or <app>/<volume>"
        ))
    })?;
    if prefix.is_empty() || vol.is_empty() {
        return Err(invalid(format!(
            "invalid source {vol_id:?}: neither part may be empty"
        )));
    }
    if prefix == "_site" {
        let path = state.driver.volume_store.site_path(vol);
        let name = SiteVolumeName::new(vol)
            .map_err(|e| invalid(format!("invalid site volume name {vol:?}: {e}")))?;
        Ok((VolumeRef::Site { name }, path))
    } else {
        let app = AppName::new(prefix)
            .map_err(|e| invalid(format!("invalid app name {prefix:?}: {e}")))?;
        let volume = AppVolumeName::new(vol)
            .map_err(|e| invalid(format!("invalid app volume name {vol:?}: {e}")))?;
        let instances = {
            let app = app.clone();
            let volume_str = volume.as_str().to_owned();
            state
                .db
                .call(move |db| {
                    crate::runtime::history::find_instances_for_group(
                        db,
                        &app,
                        crate::defs::resource::ResourceKind::Volume,
                        Some(&volume_str),
                    )
                })
                .map_err(|e| internal("failed to look up the volume's history", e))?
        };
        let inst = instances.into_iter().next().ok_or_else(|| {
            OiError::new(
                ErrorCode::NotFound,
                format!("no volume {prefix}/{vol} known to the registry"),
            )
        })?;
        let vol_name_canonical = crate::runtime::identity::VolumeName::of_instance(&inst);
        let path = state.driver.volume_store.path(&vol_name_canonical);
        Ok((VolumeRef::App { app, volume }, path))
    }
}

/// Format a [`VolumeRef`] into the `"_site/<name>"` / `"<app>/<volume>"`
/// shorthand used in JSON payloads.
fn volume_ref_display(r: &VolumeRef) -> String {
    match r {
        VolumeRef::Site { name } => format!("_site/{name}"),
        VolumeRef::App { app, volume } => format!("{app}/{volume}"),
    }
}

#[derive(Deserialize)]
pub(crate) struct MapExternalVolumeParams {
    pub app: AppName,
    pub external_name: ExternalVolumeName,
    pub target: VolumeRef,
    #[serde(default)]
    pub read_only: bool,
}

/// `not_found` unless the app is registered.
fn require_registered(state: &OiState, app: &AppName) -> Result<(), OiError> {
    if state.registry.read().get(app.as_str()).is_none() {
        return Err(OiError::new(
            ErrorCode::NotFound,
            format!("no app named {:?} is registered", app.as_str()),
        ));
    }
    Ok(())
}

/// A mapping's target, as far as the registry can say: an app target is
/// registered, declares the volume, and exports it. A site target is checked
/// where the mapping is written, in the same step.
// spec: VOLM#external-volume-mappings
fn check_mapping_target(state: &OiState, target: &VolumeRef) -> Result<(), OiError> {
    use crate::defs::resource::{Resource, ResourceKind};

    let VolumeRef::App { app, volume } = target else {
        return Ok(());
    };
    let invalid = |why: String| OiError::new(ErrorCode::RequirementsInvalid, why);
    let reg = state.registry.read();
    let entry = reg
        .get(app.as_str())
        .ok_or_else(|| invalid(format!("no app named {:?} is registered", app.as_str())))?;
    let def = entry.app.def.load();
    let exported = def
        .resources
        .iter()
        .find_map(|(id, resource)| match resource {
            Resource::Volume(vol)
                if id.kind == ResourceKind::Volume && id.name.as_str() == volume.as_str() =>
            {
                Some(vol.def.lock().exported.is_some())
            }
            _ => None,
        });
    match exported {
        None => Err(invalid(format!(
            "app {:?} declares no volume named {:?}",
            app.as_str(),
            volume.as_str()
        ))),
        Some(false) => Err(invalid(format!(
            "volume {:?} of app {:?} is not exported",
            volume.as_str(),
            app.as_str()
        ))),
        Some(true) => Ok(()),
    }
}

fn site_target_missing(target: &VolumeRef) -> OiError {
    let name = match target {
        VolumeRef::Site { name } => name.as_str(),
        VolumeRef::App { .. } => unreachable!("only a site target can be missing from the table"),
    };
    OiError::new(
        ErrorCode::RequirementsInvalid,
        format!("no site volume named {name:?}"),
    )
}

// spec: VOLM#mapping-an-external-volume
pub(crate) fn map_external_volume(
    state: &OiState,
    params: MapExternalVolumeParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::external_volume_mappings::{self, ExternalVolumeMapping, Written};

    require_registered(state, &params.app)?;

    // spec: IDN#reserved-names
    crate::reserved::check_external_volume_name(&params.external_name)
        .map_err(|e| OiError::new(ErrorCode::RequirementsInvalid, e.to_string()))?;

    check_mapping_target(state, &params.target)?;

    let app = params.app.clone();
    let external_name = params.external_name.clone();
    let event_target = params.target.clone();
    let mapping = ExternalVolumeMapping {
        app: params.app,
        external_name: params.external_name,
        target: params.target,
        read_only: params.read_only,
    };

    let target = mapping.target.clone();
    match state
        .db
        .call(move |db| external_volume_mappings::create_unique(db, &mapping))
        .map_err(|e| internal("failed to create mapping", e))?
    {
        Written::Done => {}
        Written::Duplicate | Written::Missing => {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                format!(
                    "app {:?} already has a mapping for external volume {:?}",
                    app.as_str(),
                    external_name.as_str()
                ),
            ));
        }
        Written::SiteTargetMissing => return Err(site_target_missing(&target)),
    }

    // spec: SITE#external-mappings
    ctx.events
        .external_volume_mapped(&app, &external_name, &event_target, params.read_only);

    state.tick_notify.notify_one();
    Ok(json!({ "mapped": true }))
}

#[derive(Deserialize)]
pub(crate) struct UnmapExternalVolumeParams {
    pub app: AppName,
    pub external_name: ExternalVolumeName,
}

// spec: VOLM#unmapping-an-external-volume
pub(crate) fn unmap_external_volume(
    state: &OiState,
    params: UnmapExternalVolumeParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::external_volume_mappings;

    // Check if the app is installed (volume potentially in use).
    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(params.app.as_str()) {
            let def = entry.app.def.load();
            let has_volume = def.resources.keys().any(|id| {
                id.kind == crate::defs::resource::ResourceKind::ExternalVolume
                    && params.external_name == id.name.as_str()
            });
            if has_volume {
                return Err(OiError::new(
                    ErrorCode::RequirementsInvalid,
                    format!(
                        "external volume {:?} is declared by app {:?}; \
                         uninstall the app or remove the volume reference first",
                        params.external_name, params.app
                    ),
                ));
            }
        }
    }

    let app_owned = params.app.clone();
    let external_name_owned = params.external_name.clone();
    let deleted = state
        .db
        .call(move |db| external_volume_mappings::delete(db, &app_owned, &external_name_owned))
        .map_err(|e| internal("failed to delete mapping", e))?;

    if !deleted {
        return Err(OiError::new(
            ErrorCode::NotFound,
            format!(
                "no mapping for {:?} in app {:?}",
                params.external_name, params.app
            ),
        ));
    }

    // spec: SITE#external-mappings
    ctx.events
        .external_volume_unmapped(&params.app, &params.external_name);

    // Containers stop using the volume, or fall back, on the next iteration.
    state.tick_notify.notify_one();
    Ok(json!({ "unmapped": true }))
}

// spec: VOLM#remapping-an-external-volume
pub(crate) fn remap_external_volume(
    state: &OiState,
    params: MapExternalVolumeParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::external_volume_mappings::{self, ExternalVolumeMapping, Written};

    check_mapping_target(state, &params.target)?;

    let mapping = ExternalVolumeMapping {
        app: params.app.clone(),
        external_name: params.external_name.clone(),
        target: params.target.clone(),
        read_only: params.read_only,
    };

    let (written, previous) = state
        .db
        .call(move |db| external_volume_mappings::update_existing(db, &mapping))
        .map_err(|e| internal("failed to update mapping", e))?;

    match written {
        Written::Done => {}
        Written::Missing | Written::Duplicate => {
            return Err(OiError::new(
                ErrorCode::NotFound,
                format!(
                    "no existing mapping for {:?} in app {:?}",
                    params.external_name, params.app
                ),
            ));
        }
        Written::SiteTargetMissing => return Err(site_target_missing(&params.target)),
    }

    let previous = previous.expect("a mapping that was updated had a previous one");
    // spec: SITE#external-mappings
    ctx.events.external_volume_remapped(
        &params.app,
        &params.external_name,
        seedling_protocol::events::ExternalMappingSnapshot {
            target: &params.target,
            read_only: params.read_only,
        },
        seedling_protocol::events::ExternalMappingSnapshot {
            target: &previous.target,
            read_only: previous.read_only,
        },
    );

    // Trigger reconciliation so containers pick up the new mapping.
    state.tick_notify.notify_one();
    Ok(json!({ "remapped": true }))
}

#[derive(Deserialize)]
pub(crate) struct ListExternalMappingsParams {
    pub app: Option<AppName>,
}

pub(crate) fn list_external_mappings(
    state: &OiState,
    params: ListExternalMappingsParams,
) -> HandlerResult {
    use crate::runtime::external_volume_mappings;

    let app_filter = params.app.clone();
    let mappings = state
        .db
        .call(move |db| {
            if let Some(app) = &app_filter {
                external_volume_mappings::list_for_app(db, app)
            } else {
                external_volume_mappings::list_all(db)
            }
        })
        .map_err(|e| OiError::new(ErrorCode::Internal, format!("failed to list mappings: {e}")))?;

    let items: Vec<_> = mappings
        .iter()
        .map(|m| {
            json!({
                "app": m.app,
                "external_name": m.external_name,
                "read_only": m.read_only,
                "target": m.target,
            })
        })
        .collect();

    Ok(json!(items))
}

pub(crate) fn list_declared_external_volumes(state: &OiState) -> HandlerResult {
    use crate::defs::resource::{Resource, ResourceKind};

    let reg = state.registry.read();
    let mut items: Vec<serde_json::Value> = reg
        .iter()
        .flat_map(|entry| {
            let def = entry.app.def.load();
            def.resources
                .iter()
                .filter(|(id, _)| id.kind == ResourceKind::ExternalVolume)
                .map(|(id, resource)| {
                    let mut item = json!({ "app": entry.name, "name": id.name.as_str() });
                    if let Resource::ExternalVolume(ev) = resource
                        && let Some(desc) = ev.def.lock().description.clone()
                    {
                        item["description"] = json!(desc);
                    }
                    item
                })
                .collect::<Vec<_>>()
        })
        .collect();
    items.sort_by(|a, b| {
        let ak = (
            a["app"].as_str().unwrap_or(""),
            a["name"].as_str().unwrap_or(""),
        );
        let bk = (
            b["app"].as_str().unwrap_or(""),
            b["name"].as_str().unwrap_or(""),
        );
        ak.cmp(&bk)
    });
    Ok(json!(items))
}
