use seedling_protocol::error::{ErrorCode, HandlerResult, OiError};
use seedling_protocol::names::{
    AppName, AppVolumeName, ExternalVolumeName, HeldVolumeId, SiteVolumeName, VolumeRef,
};
use serde::Deserialize;
use serde_json::json;

use crate::oi::{handler::RequestCtx, state::OiState};

pub(crate) fn list_held(state: &OiState) -> HandlerResult {
    let held = state.driver.volume_store.list_held().map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("failed to list held volumes: {e}"),
        )
    })?;

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

// r[impl actuate.volume.hold.confirm]
pub(crate) fn delete_held(
    state: &OiState,
    params: DeleteHeldParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    // confirm_delete_held is async, but OI handlers are sync.
    // Use block_in_place to run the async operation.
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            state
                .driver
                .volume_store
                .confirm_delete_held(&params.id)
                .await
                .map_err(|e| {
                    OiError::new(
                        ErrorCode::RequirementsInvalid,
                        format!("failed to delete held volume: {e}"),
                    )
                })
        })
    })?;

    // r[impl actuate.volume.hold.events]
    ctx.events.held_volume_deleted(params.id);

    Ok(json!({ "deleted": true }))
}

pub(crate) fn list_exported(state: &OiState) -> HandlerResult {
    let registry = state.registry.read();
    let mut exported = Vec::new();

    for (app_name, _status) in registry.list() {
        let Some(entry) = registry.get(app_name.as_str()) else {
            continue;
        };
        let def = entry.app.def.load();
        for (id, resource) in &def.resources {
            if let crate::defs::resource::Resource::Volume(vol) = resource {
                let vol_def = vol.def.lock();
                if let Some(export_opts) = &vol_def.exported {
                    let mut entry = json!({
                        "app": app_name,
                        "volume_name": id.name.as_str(),
                    });
                    if let Some(desc) = &export_opts.description {
                        entry["description"] = json!(desc);
                    }
                    exported.push(entry);
                }
            }
        }
    }

    Ok(json!(exported))
}

/// Every named, persistent (non-tmpfs) app volume, whether or not it is
/// exported. Used by the multi-volume shell picker so operators can pull
/// internal-only volumes into a recovery session alongside exported ones.
pub(crate) fn list_app_volumes(state: &OiState) -> HandlerResult {
    let registry = state.registry.read();
    let mut volumes = Vec::new();

    for (app_name, _status) in registry.list() {
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

// r[impl volume.site.lifecycle]
pub(crate) fn create_site_volume(
    state: &OiState,
    params: CreateSiteVolumeParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::site_volumes::{SiteVolumeDef, SiteVolumeKind};

    let kind = match params.kind.as_str() {
        "managed" => SiteVolumeKind::Managed,
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

    let def = SiteVolumeDef {
        name: params.name.clone(),
        kind: kind.clone(),
        created_at: jiff::Timestamp::now().to_string(),
    };

    // For managed volumes, create the backing storage.
    if kind == SiteVolumeKind::Managed {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                state
                    .driver
                    .volume_store
                    .create_site(params.name.as_str())
                    .await
                    .map_err(|e| {
                        OiError::new(
                            ErrorCode::Internal,
                            format!("failed to create site volume storage: {e}"),
                        )
                    })
            })
        })?;
    }

    state
        .db
        .call(move |db| crate::runtime::site_volumes::create(db, &def))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to store site volume: {e}"),
            )
        })?;

    // r[impl volume.site.lifecycle.events]
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

// r[impl volume.site.lifecycle]
pub(crate) fn delete_site_volume(
    state: &OiState,
    params: DeleteSiteVolumeParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::site_volumes::SiteVolumeKind;

    let name_owned = params.name.clone();
    let def = state
        .db
        .call(move |db| crate::runtime::site_volumes::get(db, &name_owned))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to look up site volume: {e}"),
            )
        })?;

    let def = def.ok_or_else(|| {
        OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("no site volume named {:?}", params.name.as_str()),
        )
    })?;

    // r[impl actuate.volume.hold]
    // r[impl volume.site.snapshot.delete]
    // Managed kinds carry runtime-owned data and route through the held-volume
    // mechanism so an operator must explicitly confirm the final removal.
    // Snapshot kinds are deleted directly: snapshots are read-only BTRFS
    // subvolumes (so the rename used by the hold path returns EROFS), and
    // their source volume remains intact so there is nothing to recover from
    // a held copy. Bind kinds only reference an operator-provided host path,
    // so dropping the row is sufficient.
    let held_meta = match def.kind {
        SiteVolumeKind::Managed => {
            let meta = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    state
                        .driver
                        .volume_store
                        .hold_site(params.name.as_str(), "site volume deleted by operator")
                        .await
                        .map_err(|e| {
                            OiError::new(
                                ErrorCode::Internal,
                                format!("failed to hold site volume for review: {e}"),
                            )
                        })
                })
            })?;
            Some(meta)
        }
        SiteVolumeKind::Snapshot { .. } => {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    state
                        .driver
                        .volume_store
                        .remove_site(params.name.as_str())
                        .await
                        .map_err(|e| {
                            OiError::new(
                                ErrorCode::Internal,
                                format!("failed to remove site snapshot: {e}"),
                            )
                        })
                })
            })?;
            None
        }
        SiteVolumeKind::Bind { .. } => None,
    };

    let name_owned = params.name.clone();
    state
        .db
        .call(move |db| crate::runtime::site_volumes::delete(db, &name_owned))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to delete site volume: {e}"),
            )
        })?;

    let held_id = held_meta.as_ref().map(|m| m.id);
    let kind_str = match def.kind {
        SiteVolumeKind::Managed => "managed",
        SiteVolumeKind::Bind { .. } => "bind",
        SiteVolumeKind::Snapshot { .. } => "snapshot",
    };
    if let Some(meta) = &held_meta {
        // r[impl actuate.volume.hold.events]
        //
        // Site-originated holds carry the synthetic "_site" marker in place
        // of an app name. Bypass validation so the pseudo-name round-trips
        // through the event.
        let held_app = AppName::new_unchecked(meta.app.clone());
        ctx.events
            .held_volume_created(meta.id, &held_app, &meta.volume_name, &meta.reason);
    }
    // r[impl volume.site.lifecycle.events]
    ctx.events
        .site_volume_deleted(params.name.as_str(), kind_str, held_id);

    Ok(json!({ "deleted": true }))
}

#[derive(Deserialize)]
pub(crate) struct SnapshotSiteVolumeParams {
    /// Name for the new snapshot site volume
    pub name: SiteVolumeName,
    /// Source volume ID: _site/<name> for a managed site volume, or <app>/<volume> for an app volume
    pub source: String,
}

// r[impl volume.site.snapshot]
pub(crate) fn snapshot_site_volume(
    state: &OiState,
    params: SnapshotSiteVolumeParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::site_volumes::{SiteVolumeDef, SiteVolumeKind};

    let (source, source_path) = parse_source_vol_id(&params.source, state)
        .map_err(|e| OiError::new(ErrorCode::RequirementsInvalid, e))?;

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            state
                .driver
                .volume_store
                .snapshot_site(params.name.as_str(), &source_path)
                .await
                .map_err(|e| {
                    OiError::new(
                        ErrorCode::Internal,
                        format!("failed to create snapshot: {e}"),
                    )
                })
        })
    })?;

    let event_source = source.clone();
    let def = SiteVolumeDef {
        name: params.name.clone(),
        kind: SiteVolumeKind::Snapshot { source },
        created_at: jiff::Timestamp::now().to_string(),
    };

    state
        .db
        .call(move |db| crate::runtime::site_volumes::create(db, &def))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to store snapshot site volume: {e}"),
            )
        })?;

    // r[impl volume.site.snapshot.events]
    ctx.events
        .site_volume_snapshotted(params.name.as_str(), &event_source);

    Ok(json!({ "created": true, "name": params.name }))
}

#[derive(Deserialize)]
pub(crate) struct PromoteSiteVolumeParams {
    /// Name of the existing snapshot site volume to promote
    pub source: SiteVolumeName,
    /// Name for the new managed site volume
    pub name: SiteVolumeName,
}

// r[impl volume.site.promote]
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
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to look up source site volume: {e}"),
            )
        })?;

    let source_def = source_def.ok_or_else(|| {
        OiError::new(
            ErrorCode::RequirementsInvalid,
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

    let source_path = state.driver.volume_store.site_path(params.source.as_str());

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            state
                .driver
                .volume_store
                .promote_site_snapshot(params.name.as_str(), &source_path)
                .await
                .map_err(|e| {
                    OiError::new(
                        ErrorCode::Internal,
                        format!("failed to promote snapshot: {e}"),
                    )
                })
        })
    })?;

    let def = SiteVolumeDef {
        name: params.name.clone(),
        kind: SiteVolumeKind::Managed,
        created_at: jiff::Timestamp::now().to_string(),
    };

    state
        .db
        .call(move |db| crate::runtime::site_volumes::create(db, &def))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to store promoted site volume: {e}"),
            )
        })?;

    // r[impl volume.site.promote.events]
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
fn parse_source_vol_id(
    vol_id: &str,
    state: &OiState,
) -> Result<(VolumeRef, std::path::PathBuf), String> {
    let (prefix, vol) = vol_id.split_once('/').ok_or_else(|| {
        format!("invalid source {vol_id:?}: expected _site/<name> or <app>/<volume>")
    })?;
    if prefix.is_empty() || vol.is_empty() {
        return Err(format!(
            "invalid source {vol_id:?}: neither part may be empty"
        ));
    }
    if prefix == "_site" {
        let path = state.driver.volume_store.site_path(vol);
        let name = SiteVolumeName::new(vol)
            .map_err(|e| format!("invalid site volume name {vol:?}: {e}"))?;
        Ok((VolumeRef::Site { name }, path))
    } else {
        let app = AppName::new(prefix).map_err(|e| format!("invalid app name {prefix:?}: {e}"))?;
        let volume =
            AppVolumeName::new(vol).map_err(|e| format!("invalid app volume name {vol:?}: {e}"))?;
        let instances = {
            let app = app.clone();
            let volume_str = volume.as_str().to_owned();
            state.db.call(move |db| {
                crate::runtime::history::find_instances_for_group(
                    db,
                    &app,
                    crate::defs::resource::ResourceKind::Volume,
                    Some(&volume_str),
                )
                .unwrap_or_default()
            })
        };
        let inst = instances
            .into_iter()
            .next()
            .ok_or_else(|| format!("no volume {prefix}/{vol} known to the registry"))?;
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

pub(crate) fn map_external_volume(
    state: &OiState,
    params: MapExternalVolumeParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::external_volume_mappings::{self, ExternalVolumeMapping};

    let app = params.app.clone();
    let external_name = params.external_name.clone();
    let event_target = params.target.clone();
    let mapping = ExternalVolumeMapping {
        app: params.app,
        external_name: params.external_name,
        target: params.target,
        read_only: params.read_only,
    };

    state
        .db
        .call(move |db| external_volume_mappings::create(db, &mapping))
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to create mapping: {e}"),
            )
        })?;

    // r[impl volume.external.mapping.events]
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
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to delete mapping: {e}"),
            )
        })?;

    if !deleted {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "no mapping for {:?} in app {:?}",
                params.external_name, params.app
            ),
        ));
    }

    // r[impl volume.external.mapping.events]
    ctx.events
        .external_volume_unmapped(&params.app, &params.external_name);

    Ok(json!({ "unmapped": true }))
}

pub(crate) fn remap_external_volume(
    state: &OiState,
    params: MapExternalVolumeParams,
    ctx: &RequestCtx,
) -> HandlerResult {
    use crate::runtime::external_volume_mappings::{self, ExternalVolumeMapping};

    let mapping = ExternalVolumeMapping {
        app: params.app.clone(),
        external_name: params.external_name.clone(),
        target: params.target.clone(),
        read_only: params.read_only,
    };

    let app_for_prev = params.app.clone();
    let external_name_for_prev = params.external_name.clone();
    let (updated, previous) = state
        .db
        .call(move |db| {
            let prev = external_volume_mappings::get(db, &app_for_prev, &external_name_for_prev)?;
            let updated = external_volume_mappings::update(db, &mapping)?;
            Ok::<_, rusqlite::Error>((updated, prev))
        })
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("failed to update mapping: {e}"),
            )
        })?;

    if !updated {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!(
                "no existing mapping for {:?} in app {:?}",
                params.external_name, params.app
            ),
        ));
    }

    let previous = previous.expect("update succeeded so prior row existed");
    // r[impl volume.external.mapping.events]
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
    use crate::defs::resource::ResourceKind;

    let reg = state.registry.read();
    let mut items: Vec<serde_json::Value> = reg
        .iter()
        .flat_map(|entry| {
            let def = entry.app.def.load();
            def.resources
                .keys()
                .filter(|id| id.kind == ResourceKind::ExternalVolume)
                .map(|id| json!({ "app": entry.name, "name": id.name.as_str() }))
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
