//! OI handlers for leader-side grove operations and read-only status.
//!
//! All mutating handlers load the leader signing key from the transport
//! key path on every call (the key is small, file IO is cheap, and not
//! caching it keeps the live grove state free of long-lived signing
//! material). Mutations route through [`crate::grove::GroveState::publish`]
//! or `init`; structural errors (not-leader, payload-too-large, etc.)
//! map to typed [`ErrorCode`] values so the CLI and web surfaces can
//! present them.

use serde::Deserialize;
use serde_json::{Value, json};

use seedling_protocol::error::{ErrorCode, OiError};
use seedling_protocol::grove::{Member, Param};
use seedling_protocol::keys;

use crate::grove::publish::PublishError;
use crate::oi::state::OiState;

use super::HandlerResult;

#[derive(Deserialize)]
pub(crate) struct InitParams {
    /// Human-readable label for the leader's own membership entry.
    /// Defaults to `"leader"` if omitted.
    #[serde(default = "default_leader_label")]
    pub label: String,
}

fn default_leader_label() -> String {
    "leader".to_owned()
}

#[derive(Deserialize)]
pub(crate) struct InviteParams {
    pub fingerprint: String,
    pub label: String,
}

#[derive(Deserialize)]
pub(crate) struct RevokeParams {
    pub fingerprint: String,
}

#[derive(Deserialize)]
pub(crate) struct ParamSetParams {
    pub name: String,
    pub kind: String,
    pub value: String,
    /// Reserved for future versions; setting `secret = true` is rejected
    /// with [`ErrorCode::RequirementsInvalid`] in this protocol version.
    #[serde(default)]
    pub secret: bool,
}

#[derive(Deserialize)]
pub(crate) struct ParamUnsetParams {
    pub name: String,
}

fn grove(state: &OiState) -> Result<&crate::grove::GroveState, OiError> {
    state
        .grove
        .get()
        .map(|arc| arc.as_ref())
        .ok_or_else(|| OiError::new(ErrorCode::ServerBusy, "grove state not yet initialised"))
}

fn load_leader_key(state: &OiState) -> Result<ed25519_dalek::SigningKey, OiError> {
    keys::load_or_generate(&state.transport.key_path).map_err(|e| {
        OiError::new(
            ErrorCode::Internal,
            format!("loading leader signing key: {e}"),
        )
    })
}

fn publish_error_to_oi(e: PublishError) -> OiError {
    let (code, msg) = match e {
        PublishError::NotMember => (
            ErrorCode::NotFound,
            "node is not a member of any grove".to_owned(),
        ),
        PublishError::NotLeader => (
            ErrorCode::RequirementsInvalid,
            "this operation requires the leader role".to_owned(),
        ),
        PublishError::AlreadyMember => (
            ErrorCode::AlreadyInstalled,
            "node is already a member of a grove".to_owned(),
        ),
        PublishError::LeaderKeyMismatch { expected, actual } => (
            ErrorCode::Internal,
            format!("leader key fingerprint {actual} does not match pinned leader {expected}"),
        ),
        PublishError::PayloadTooLarge {
            current_bytes,
            cap_bytes,
        } => (
            ErrorCode::RequirementsInvalid,
            format!(
                "next payload would be {current_bytes} bytes, exceeding the {cap_bytes}-byte publish cap"
            ),
        ),
        PublishError::SecretsNotSupported => (
            ErrorCode::RequirementsInvalid,
            "secret grove parameters are not supported in this protocol version".to_owned(),
        ),
        PublishError::Sign(e) => (ErrorCode::Internal, format!("payload sign: {e}")),
        PublishError::Db(e) => (ErrorCode::Internal, format!("grove db: {e}")),
    };
    OiError::new(code, msg)
}

// g[impl bootstrap.init]
pub(crate) fn init(state: &OiState, params: InitParams) -> HandlerResult {
    let key = load_leader_key(state)?;
    let signed = grove(state)?
        .init(&key, params.label)
        .map_err(publish_error_to_oi)?;
    Ok(json!({
        "grove_id": signed.payload.grove_id,
        "seq": signed.payload.seq,
        "leader_fp": signed.payload.leader_fp,
    }))
}

// g[impl membership.invite]
pub(crate) fn invite(state: &OiState, params: InviteParams) -> HandlerResult {
    let key = load_leader_key(state)?;
    let g = grove(state)?;
    let fingerprint = params.fingerprint.clone();
    let label = params.label.clone();

    {
        // Reject obvious duplicates with a clean error rather than letting
        // the publish go through with a duplicate member entry.
        let cur = g.current.read();
        if let Some(s) = cur.as_ref()
            && s.payload.members.iter().any(|m| m.fp == fingerprint)
        {
            return Err(OiError::new(
                ErrorCode::AlreadyInstalled,
                format!("fingerprint already a member: {fingerprint}"),
            ));
        }
    }

    let signed = g
        .publish(&key, |p| {
            p.members.push(Member {
                fp: fingerprint,
                label,
            });
        })
        .map_err(publish_error_to_oi)?;
    Ok(json!({
        "seq": signed.payload.seq,
        "members_count": signed.payload.members.len(),
    }))
}

// g[impl membership.revoke]
pub(crate) fn revoke(state: &OiState, params: RevokeParams) -> HandlerResult {
    let key = load_leader_key(state)?;
    let g = grove(state)?;
    let fingerprint = params.fingerprint.clone();

    {
        let cur = g.current.read();
        let s = cur.as_ref().ok_or_else(|| {
            OiError::new(ErrorCode::NotFound, "node is not a member of any grove")
        })?;
        if !s.payload.members.iter().any(|m| m.fp == fingerprint) {
            return Err(OiError::not_found(format!("not a member: {fingerprint}")));
        }
        if fingerprint == s.payload.leader_fp {
            return Err(OiError::new(
                ErrorCode::RequirementsInvalid,
                "cannot revoke the leader's own membership",
            ));
        }
    }

    let signed = g
        .publish(&key, |p| {
            p.members.retain(|m| m.fp != fingerprint);
        })
        .map_err(publish_error_to_oi)?;
    Ok(json!({
        "seq": signed.payload.seq,
        "members_count": signed.payload.members.len(),
    }))
}

// g[impl params.set]
pub(crate) fn param_set(state: &OiState, params: ParamSetParams) -> HandlerResult {
    if params.secret {
        return Err(OiError::new(
            ErrorCode::RequirementsInvalid,
            "secret grove parameters are not supported in this protocol version",
        ));
    }
    let key = load_leader_key(state)?;
    let name = params.name.clone();
    let kind = params.kind.clone();
    let value = params.value.clone();
    let signed = grove(state)?
        .publish(&key, |p| {
            if let Some(existing) = p.params.iter_mut().find(|q| q.name == name) {
                existing.kind = kind;
                existing.value = value;
            } else {
                p.params.push(Param { name, kind, value });
            }
        })
        .map_err(publish_error_to_oi)?;
    Ok(json!({ "seq": signed.payload.seq }))
}

// g[impl params.set]
pub(crate) fn param_unset(state: &OiState, params: ParamUnsetParams) -> HandlerResult {
    let key = load_leader_key(state)?;
    let g = grove(state)?;
    let name = params.name.clone();
    {
        let cur = g.current.read();
        let s = cur.as_ref().ok_or_else(|| {
            OiError::new(ErrorCode::NotFound, "node is not a member of any grove")
        })?;
        if !s.payload.params.iter().any(|q| q.name == name) {
            return Err(OiError::not_found(format!("grove param not set: {name}")));
        }
    }
    let signed = g
        .publish(&key, |p| {
            p.params.retain(|q| q.name != name);
        })
        .map_err(publish_error_to_oi)?;
    Ok(json!({ "seq": signed.payload.seq }))
}

// g[impl identity]
pub(crate) fn status(state: &OiState) -> HandlerResult {
    let g = grove(state)?;
    let cur = g.current.read();
    Ok(match cur.as_ref() {
        None => json!({ "is_member": false }),
        Some(s) => json!({
            "is_member": true,
            "grove_id": s.payload.grove_id,
            "leader_fp": s.payload.leader_fp,
            "current_seq": s.payload.seq,
            "members_count": s.payload.members.len(),
            "params_count": s.payload.params.len(),
        }),
    })
}

pub(crate) fn members(state: &OiState) -> HandlerResult {
    let g = grove(state)?;
    let cur = g.current.read();
    let list: Vec<Value> = match cur.as_ref() {
        None => Vec::new(),
        Some(s) => s
            .payload
            .members
            .iter()
            .map(|m| json!({ "fingerprint": m.fp, "label": m.label }))
            .collect(),
    };
    Ok(json!(list))
}

pub(crate) fn params(state: &OiState) -> HandlerResult {
    let g = grove(state)?;
    let cur = g.current.read();
    let list: Vec<Value> = match cur.as_ref() {
        None => Vec::new(),
        Some(s) => s
            .payload
            .params
            .iter()
            .map(|p| json!({ "name": p.name, "kind": p.kind, "value": p.value }))
            .collect(),
    };
    Ok(json!(list))
}
