use serde::Deserialize;
use serde_json::json;

use seedling_protocol::error::{ErrorCode, HandlerResult, OiError};

use crate::oi::state::OiState;

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
    pub id: String,
}

// r[impl actuate.volume.hold.confirm]
pub(crate) fn delete_held(state: &OiState, params: DeleteHeldParams) -> HandlerResult {
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

    Ok(json!({ "deleted": true }))
}

pub(crate) fn list_exported(state: &OiState) -> HandlerResult {
    let registry = state.registry.read();
    let mut exported = Vec::new();

    for (app_name, _status) in registry.list() {
        let Some(entry) = registry.get(&app_name) else {
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

#[derive(Deserialize)]
pub(crate) struct CreateSiteVolumeParams {
    pub name: String,
    /// "managed" or "bind"
    pub kind: String,
    /// Required when kind is "bind"
    pub host_path: Option<String>,
}

// r[impl volume.site.lifecycle]
pub(crate) fn create_site_volume(state: &OiState, params: CreateSiteVolumeParams) -> HandlerResult {
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
                    .create_site(&params.name)
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

    Ok(json!({ "created": true }))
}

pub(crate) fn list_site_volumes(state: &OiState) -> HandlerResult {
    use crate::runtime::site_volumes::SiteVolumeKind;

    let volumes = state
        .db
        .call(|db| crate::runtime::site_volumes::list(db))
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
            if let SiteVolumeKind::Snapshot {
                source_app,
                source_volume,
            } = &v.kind
            {
                if let Some(app) = source_app {
                    obj["source"] = json!(format!("{app}/{source_volume}"));
                } else {
                    obj["source"] = json!(format!("_site/{source_volume}"));
                }
            }
            obj
        })
        .collect();

    Ok(json!(items))
}

#[derive(Deserialize)]
pub(crate) struct DeleteSiteVolumeParams {
    pub name: String,
}

// r[impl volume.site.lifecycle]
pub(crate) fn delete_site_volume(state: &OiState, params: DeleteSiteVolumeParams) -> HandlerResult {
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
            format!("no site volume named {:?}", params.name),
        )
    })?;

    // For managed volumes, remove the backing storage.
    if def.kind == crate::runtime::site_volumes::SiteVolumeKind::Managed {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                state
                    .driver
                    .volume_store
                    .remove_site(&params.name)
                    .await
                    .map_err(|e| {
                        OiError::new(
                            ErrorCode::Internal,
                            format!("failed to remove site volume storage: {e}"),
                        )
                    })
            })
        })?;
    }

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

    Ok(json!({ "deleted": true }))
}

#[derive(Deserialize)]
pub(crate) struct SnapshotSiteVolumeParams {
    /// Name for the new snapshot site volume
    pub name: String,
    /// Source volume ID: _site/<name> for a managed site volume, or <app>/<volume> for an app volume
    pub source: String,
}

// r[impl volume.site.snapshot]
pub(crate) fn snapshot_site_volume(
    state: &OiState,
    params: SnapshotSiteVolumeParams,
) -> HandlerResult {
    use crate::runtime::site_volumes::{SiteVolumeDef, SiteVolumeKind};

    let (source_app, source_volume, source_path) =
        parse_source_vol_id(&params.source, &state.driver.volume_store)
            .map_err(|e| OiError::new(ErrorCode::RequirementsInvalid, e))?;

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            state
                .driver
                .volume_store
                .snapshot_site(&params.name, &source_path)
                .await
                .map_err(|e| {
                    OiError::new(
                        ErrorCode::Internal,
                        format!("failed to create snapshot: {e}"),
                    )
                })
        })
    })?;

    let def = SiteVolumeDef {
        name: params.name.clone(),
        kind: SiteVolumeKind::Snapshot {
            source_app,
            source_volume,
        },
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

    Ok(json!({ "created": true, "name": params.name }))
}

fn parse_source_vol_id(
    vol_id: &str,
    vol_store: &crate::system::volume_store::VolumeStore,
) -> Result<(Option<String>, String, std::path::PathBuf), String> {
    let (prefix, vol) = vol_id.split_once('/').ok_or_else(|| {
        format!("invalid source {vol_id:?}: expected _site/<name> or <app>/<volume>")
    })?;
    if prefix.is_empty() || vol.is_empty() {
        return Err(format!(
            "invalid source {vol_id:?}: neither part may be empty"
        ));
    }
    if prefix == "_site" {
        let path = vol_store.site_path(vol);
        Ok((None, vol.to_owned(), path))
    } else {
        let vol_name = format!("{prefix}-{vol}");
        let path = vol_store.path(&vol_name);
        Ok((Some(prefix.to_owned()), vol.to_owned(), path))
    }
}

#[derive(Deserialize)]
pub(crate) struct MapExternalVolumeParams {
    pub app: String,
    pub external_name: String,
    /// "exported" or "site"
    pub target_kind: String,
    /// Required when target_kind is "exported"
    pub target_app: Option<String>,
    pub target_volume: String,
    #[serde(default)]
    pub read_only: bool,
}

pub(crate) fn map_external_volume(
    state: &OiState,
    params: MapExternalVolumeParams,
) -> HandlerResult {
    use crate::runtime::external_volume_mappings::{self, ExternalVolumeMapping};

    let target = parse_mapping_target(
        &params.target_kind,
        params.target_app.as_deref(),
        &params.target_volume,
    )?;

    let mapping = ExternalVolumeMapping {
        app: params.app,
        external_name: params.external_name,
        target,
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

    state.tick_notify.notify_one();
    Ok(json!({ "mapped": true }))
}

#[derive(Deserialize)]
pub(crate) struct UnmapExternalVolumeParams {
    pub app: String,
    pub external_name: String,
}

pub(crate) fn unmap_external_volume(
    state: &OiState,
    params: UnmapExternalVolumeParams,
) -> HandlerResult {
    use crate::runtime::external_volume_mappings;

    // Check if the app is installed (volume potentially in use).
    {
        let reg = state.registry.read();
        if let Some(entry) = reg.get(&params.app) {
            let def = entry.app.def.load();
            let has_volume = def.resources.keys().any(|id| {
                id.kind == crate::defs::resource::ResourceKind::ExternalVolume
                    && id.name.as_str() == params.external_name
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

    Ok(json!({ "unmapped": true }))
}

pub(crate) fn remap_external_volume(
    state: &OiState,
    params: MapExternalVolumeParams,
) -> HandlerResult {
    use crate::runtime::external_volume_mappings::{self, ExternalVolumeMapping};

    let target = parse_mapping_target(
        &params.target_kind,
        params.target_app.as_deref(),
        &params.target_volume,
    )?;

    let mapping = ExternalVolumeMapping {
        app: params.app.clone(),
        external_name: params.external_name.clone(),
        target,
        read_only: params.read_only,
    };

    let updated = state
        .db
        .call(move |db| external_volume_mappings::update(db, &mapping))
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

    // Trigger reconciliation so containers pick up the new mapping.
    state.tick_notify.notify_one();
    Ok(json!({ "remapped": true }))
}

#[derive(Deserialize)]
pub(crate) struct ListExternalMappingsParams {
    pub app: Option<String>,
}

pub(crate) fn list_external_mappings(
    state: &OiState,
    params: ListExternalMappingsParams,
) -> HandlerResult {
    use crate::runtime::external_volume_mappings::{self, MappingTarget};

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
            let mut obj = json!({
                "app": m.app,
                "external_name": m.external_name,
                "read_only": m.read_only,
            });
            match &m.target {
                MappingTarget::Exported {
                    target_app,
                    target_volume,
                } => {
                    obj["target_kind"] = json!("exported");
                    obj["target_app"] = json!(target_app);
                    obj["target_volume"] = json!(target_volume);
                }
                MappingTarget::Site { target_volume } => {
                    obj["target_kind"] = json!("site");
                    obj["target_volume"] = json!(target_volume);
                }
            }
            obj
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

fn parse_mapping_target(
    kind: &str,
    target_app: Option<&str>,
    target_volume: &str,
) -> Result<crate::runtime::external_volume_mappings::MappingTarget, OiError> {
    use crate::runtime::external_volume_mappings::MappingTarget;

    match kind {
        "exported" => {
            let app = target_app.ok_or_else(|| {
                OiError::new(
                    ErrorCode::RequirementsInvalid,
                    "target_app is required for exported volume mappings".to_string(),
                )
            })?;
            Ok(MappingTarget::Exported {
                target_app: app.to_owned(),
                target_volume: target_volume.to_owned(),
            })
        }
        "site" => Ok(MappingTarget::Site {
            target_volume: target_volume.to_owned(),
        }),
        other => Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            format!("invalid target_kind: {other:?}, expected \"exported\" or \"site\""),
        )),
    }
}
