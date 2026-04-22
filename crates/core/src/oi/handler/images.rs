use std::collections::BTreeMap;

use jiff::Timestamp;
use seedling_protocol::{
    error::{ErrorCode, HandlerResult, OiError},
    names::AppName,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{oi::state::OiState, runtime::images};

// ---------------------------------------------------------------------------
// /images/list
// ---------------------------------------------------------------------------

// i[image.list]
pub(crate) fn list_images(state: &OiState) -> HandlerResult {
    let (images, in_use_ids) = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            let images = state.container_runtime.list_images().await.map_err(|e| {
                OiError::new(ErrorCode::NotFound, format!("list_images failed: {e}"))
            })?;

            let mut in_use_ids: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let running = state
                .container_runtime
                // Unfiltered: every running container (workload, shell, Caddy,
                // resolver, or operator-started) keeps its image in use.
                .list(crate::system::types::ContainerFilter::default())
                .await
                .unwrap_or_default();
            for c in running {
                if let Ok(Some(s)) = state.container_runtime.inspect(&c.name).await
                    && let Some(id) = s.image_id
                {
                    in_use_ids.insert(id);
                }
            }
            Ok::<_, OiError>((images, in_use_ids))
        })
    })?;

    // Build reference → pinning-apps map in one DB call.
    let references: Vec<String> = images
        .iter()
        .flat_map(|img| img.references.iter().cloned())
        .collect();
    let pinned_map = state.db.call(move |db| {
        images::list_pinned_apps_for_references(
            db,
            &references.iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .unwrap_or_default()
    });

    // Pull tracking rows once to enrich last_used_at.
    let tracking: BTreeMap<String, i64> = {
        let ids: Vec<String> = images.iter().map(|i| i.image_id.clone()).collect();
        state.db.call(move |db| {
            let mut out = BTreeMap::new();
            for id in ids {
                if let Ok(Some(row)) = images::get_tracking(db, &id) {
                    out.insert(id, row.last_used_at);
                }
            }
            out
        })
    };

    let mut out: Vec<Value> = Vec::with_capacity(images.len());
    for img in images {
        let pinned_by: Vec<AppName> = img
            .references
            .iter()
            .flat_map(|r| pinned_map.get(r).cloned().unwrap_or_default())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();

        let created_at = rfc3339_from_secs(img.created_at_secs);
        let last_used_at = tracking
            .get(&img.image_id)
            .map(|ms| rfc3339_from_millis(*ms))
            .unwrap_or_else(|| created_at.clone());

        out.push(json!({
            "image_id": img.image_id,
            "references": img.references,
            "size_bytes": img.size_bytes,
            "created_at": created_at,
            "last_used_at": last_used_at,
            "in_use": in_use_ids.contains(&img.image_id),
            "pinned_by": pinned_by.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
        }));
    }

    Ok(json!({ "images": out }))
}

fn rfc3339_from_secs(secs: i64) -> String {
    Timestamp::from_second(secs.max(0))
        .map(|ts| ts.to_string())
        .unwrap_or_else(|_| Timestamp::UNIX_EPOCH.to_string())
}

fn rfc3339_from_millis(ms: i64) -> String {
    Timestamp::from_millisecond(ms.max(0))
        .map(|ts| ts.to_string())
        .unwrap_or_else(|_| Timestamp::UNIX_EPOCH.to_string())
}

// ---------------------------------------------------------------------------
// /images/pull
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct PullParams {
    pub reference: String,
    #[serde(default)]
    pub app: Option<String>,
}

// i[image.pull]
pub(crate) fn pull_image(state: &OiState, params: PullParams) -> HandlerResult {
    if params.reference.trim().is_empty() {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            "reference must not be empty".to_string(),
        ));
    }

    if let Some(app_str) = params.app.as_deref() {
        let app = AppName::new(app_str).map_err(|e| {
            OiError::new(ErrorCode::RequirementsInvalid, format!("invalid app: {e}"))
        })?;
        if state.registry.read().get(app.as_str()).is_none() {
            return Err(OiError::not_found(format!("app not registered: {app}")));
        }
        let reference_for_pin = params.reference.clone();
        let app_for_pin = app.clone();
        state.db.call(move |db| {
            if let Err(e) = images::upsert_pin(db, &app_for_pin, &reference_for_pin) {
                tracing::warn!(error = %e, "pull_image: failed to upsert pin");
            }
        });
    }

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            state
                .container_runtime
                .pull_image(&params.reference)
                .await
                .map_err(|e| OiError::new(ErrorCode::NotFound, format!("pull failed: {e}")))
        })
    })?;

    Ok(json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// /images/remove
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct RemoveParams {
    pub reference: String,
    #[serde(default)]
    pub force: bool,
}

// i[image.remove]
pub(crate) fn remove_image(state: &OiState, params: RemoveParams) -> HandlerResult {
    if params.reference.trim().is_empty() {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            "reference must not be empty".to_string(),
        ));
    }

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            // If any running container uses this reference and force is not
            // set, refuse — operators shouldn't accidentally detach a live
            // workload from its image.
            if !params.force {
                let target_id = state
                    .container_runtime
                    .local_image_id(&params.reference)
                    .await
                    .ok()
                    .flatten();
                if let Some(target_id) = target_id.as_deref() {
                    let running = state
                        .container_runtime
                        .list(crate::system::types::ContainerFilter::default())
                        .await
                        .unwrap_or_default();
                    for c in running {
                        if let Ok(Some(s)) = state.container_runtime.inspect(&c.name).await
                            && let Some(id) = s.image_id.as_deref()
                            && id == target_id
                        {
                            return Err(OiError::new(
                                ErrorCode::RequirementsInvalid,
                                "image is in use by a running container; pass force=true to remove anyway"
                                    .to_string(),
                            ));
                        }
                    }
                }
            }

            let reference = params.reference.clone();
            state.db.call(move |db| {
                let _ = images::clear_pins_by_reference(db, &reference);
            });

            let removed = state
                .container_runtime
                .remove_image(&params.reference, params.force)
                .await
                .map_err(|e| {
                    OiError::new(ErrorCode::NotFound, format!("remove_image failed: {e}"))
                })?;
            if !removed {
                return Err(OiError::not_found(format!(
                    "image not found locally: {}",
                    params.reference
                )));
            }
            Ok(json!({ "ok": true }))
        })
    })
}

// ---------------------------------------------------------------------------
// /images/pins/list
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct PinsListParams {
    #[serde(default)]
    pub app: Option<String>,
}

// i[image.pin.list]
pub(crate) fn list_pins(state: &OiState, params: PinsListParams) -> HandlerResult {
    let app: Option<AppName> = match params.app {
        Some(s) => Some(AppName::new(&s).map_err(|e| {
            OiError::new(ErrorCode::RequirementsInvalid, format!("invalid app: {e}"))
        })?),
        None => None,
    };
    let app_for_query = app.clone();
    let pins = state
        .db
        .call(move |db| images::list_pins(db, app_for_query.as_ref()).unwrap_or_default());
    let arr: Vec<Value> = pins
        .into_iter()
        .map(|p| {
            json!({
                "app": p.app.as_str(),
                "reference": p.reference,
                "pinned_at": rfc3339_from_millis(p.pinned_at),
            })
        })
        .collect();
    Ok(json!({ "pins": arr }))
}

// ---------------------------------------------------------------------------
// /images/pins/clear
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct PinsClearParams {
    pub app: String,
    #[serde(default)]
    pub reference: Option<String>,
}

// i[image.pin.clear]
pub(crate) fn clear_pins(state: &OiState, params: PinsClearParams) -> HandlerResult {
    let app = AppName::new(&params.app)
        .map_err(|e| OiError::new(ErrorCode::RequirementsInvalid, format!("invalid app: {e}")))?;
    state.db.call(move |db| match params.reference {
        Some(r) => {
            let _ = images::clear_pin(db, &app, &r);
        }
        None => {
            let _ = images::clear_pins_for_app(db, &app);
        }
    });
    Ok(json!({ "ok": true }))
}
