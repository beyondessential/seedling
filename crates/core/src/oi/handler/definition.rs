//! Turning the definition a request supplies into a bundle and its source.

use std::sync::Arc;

use seedling_protocol::error::{ErrorCode, OiError};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    oi::{handler::RequestCtx, state::OiState},
    runtime::definition::{
        Bundle, Origin, Source,
        fetch::{self, DefinitionRef, FetchError},
        version,
    },
};

/// The definition fields a request may carry; exactly one of `script`,
/// `bundle`, and `reference`.
// i[impl definition.source]
#[derive(Deserialize, Default)]
pub(crate) struct DefinitionInput {
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub bundle: Option<Map<String, Value>>,
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub origin: Option<Origin>,
}

impl DefinitionInput {
    pub fn is_empty(&self) -> bool {
        self.script.is_none() && self.bundle.is_none() && self.reference.is_none()
    }
}

/// A definition ready to be checked and evaluated.
pub(crate) struct Resolved {
    pub bundle: Arc<Bundle>,
    pub source: Source,
}

fn requirements(msg: impl Into<String>) -> OiError {
    OiError::new(ErrorCode::RequirementsInvalid, msg)
}

/// Run a registry call to completion from a synchronous handler.
pub(crate) fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(f))
}

/// The registry allowlist, read fresh so a change applies to the next fetch.
fn allowed_registries(state: &OiState) -> Result<Vec<String>, OiError> {
    state
        .db
        .call(crate::runtime::registries::list_allowed_registries)
        .map_err(|e| {
            OiError::new(
                ErrorCode::Internal,
                format!("could not read the registry allowlist: {e}"),
            )
        })
}

/// Parse a reference and check it against the allowlist, before any request.
// i[impl definition.fetch.access]
pub(crate) fn allowed_reference(state: &OiState, raw: &str) -> Result<DefinitionRef, OiError> {
    let reference = DefinitionRef::parse(raw)?;
    fetch::check_allowed(&allowed_registries(state)?, &reference)?;
    Ok(reference)
}

/// Resolve the supplied definition without storing anything: decode a
/// pushed bundle, or fetch one. Nothing else about it is checked here; the
/// caller decides whether the running Seedling must support it.
// i[impl definition.source]
// i[impl definition.fetch]
pub(crate) fn resolve(
    state: &OiState,
    input: DefinitionInput,
    ctx: &RequestCtx,
) -> Result<Resolved, OiError> {
    let DefinitionInput {
        script,
        bundle,
        reference,
        origin,
    } = input;
    let supplied = [script.is_some(), bundle.is_some(), reference.is_some()]
        .into_iter()
        .filter(|s| *s)
        .count();
    if supplied != 1 {
        return Err(requirements(
            "supply exactly one of `script`, `bundle`, or `reference`",
        ));
    }
    if let Some(raw) = reference {
        if origin.is_some() {
            return Err(requirements(
                "`origin` describes a pushed definition and cannot accompany `reference`",
            ));
        }
        let reference = allowed_reference(state, &raw)?;
        let fetched = block_on(fetch::fetch(
            state.definition_registry.as_ref(),
            &reference,
            &version::running(),
        ))
        .inspect_err(|e| {
            if matches!(e, FetchError::Failed(_)) {
                tracing::warn!(reference = %raw, "definition fetch failed: {e}");
            }
        })?;
        return Ok(Resolved {
            bundle: Arc::new(fetched.bundle),
            source: Source::Fetched {
                reference: raw,
                digest: fetched.digest,
            },
        });
    }
    let bundle = match (script, bundle) {
        (Some(text), None) => Bundle::from_script(&text)?,
        (None, Some(map)) => Bundle::from_wire(&map)?,
        _ => unreachable!("exactly one was supplied"),
    };
    Ok(Resolved {
        bundle: Arc::new(bundle),
        source: Source::Pushed {
            pushed_by: Some((*ctx.events.actor).clone()),
            reported_origin: origin,
        },
    })
}

#[cfg(test)]
mod tests;
