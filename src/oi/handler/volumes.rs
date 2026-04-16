use serde::Deserialize;
use serde_json::json;

use crate::oi::{
    error::{ErrorCode, HandlerResult, OiError},
    state::OiState,
};

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
        let def = entry.app.def.lock();
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
    #[serde(default)]
    pub read_only: bool,
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
        read_only: params.read_only,
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

    let db = state.db.lock();
    crate::runtime::site_volumes::create(&db, &def).map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("failed to store site volume: {e}"),
        )
    })?;

    Ok(json!({ "created": true }))
}

pub(crate) fn list_site_volumes(state: &OiState) -> HandlerResult {
    use crate::runtime::site_volumes::SiteVolumeKind;

    let db = state.db.lock();
    let volumes = crate::runtime::site_volumes::list(&db).map_err(|e| {
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
                },
                "read_only": v.read_only,
                "created_at": v.created_at,
            });
            if let SiteVolumeKind::Bind { host_path } = &v.kind {
                obj["host_path"] = json!(host_path);
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
    let db = state.db.lock();
    let def = crate::runtime::site_volumes::get(&db, &params.name).map_err(|e| {
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

    crate::runtime::site_volumes::delete(&db, &params.name).map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("failed to delete site volume: {e}"),
        )
    })?;

    Ok(json!({ "deleted": true }))
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
    };

    let db = state.db.lock();
    external_volume_mappings::create(&db, &mapping).map_err(|e| {
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
            let def = entry.app.def.lock();
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

    let db = state.db.lock();
    let deleted = external_volume_mappings::delete(&db, &params.app, &params.external_name)
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
    };

    let db = state.db.lock();
    let updated = external_volume_mappings::update(&db, &mapping).map_err(|e| {
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

    let db = state.db.lock();
    let mappings = if let Some(app) = &params.app {
        external_volume_mappings::list_for_app(&db, app)
    } else {
        external_volume_mappings::list_all(&db)
    }
    .map_err(|e| OiError::new(ErrorCode::Internal, format!("failed to list mappings: {e}")))?;

    let items: Vec<_> = mappings
        .iter()
        .map(|m| {
            let mut obj = json!({
                "app": m.app,
                "external_name": m.external_name,
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
