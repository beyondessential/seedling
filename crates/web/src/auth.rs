use std::sync::Arc;

use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;

use seedling_protocol::actor::Actor;

pub mod password;
pub mod tailscale;

use crate::state::{self, AppState};

#[derive(Deserialize)]
pub struct ConnectRequest {
    pub token: Option<String>,
    pub password: Option<String>,
}

#[derive(Serialize)]
pub struct ConnectResponse {
    pub token: String,
    pub actor: Actor,
    pub wt_url: String,
    pub cert_hashes: Vec<String>,
}

// w[auth.connect]
pub async fn handle_connect(
    state: AppState,
    headers: HeaderMap,
    host: Option<String>,
    body: ConnectRequest,
) -> Response {
    let maybe_actor = resolve_actor(&state, &headers, &body);

    let actor = match maybe_actor {
        Some(a) => a,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "auth_required": "password" })),
            )
                .into_response();
        }
    };

    let session_token =
        password::issue_session_token(&state.sessions, Arc::clone(&actor), state.session_lifetime);

    let wt_token = state::issue_wt_token(&state.wt_tokens, Arc::clone(&actor));

    let wt_host = host.unwrap_or_else(|| "localhost".to_owned());
    // Strip any port from the host header to reconstruct with wt_port.
    let wt_hostname = wt_host.split(':').next().unwrap_or("localhost");
    let wt_url = format!("https://{wt_hostname}:{}/wt?t={wt_token}", state.wt_port);

    let cert_hashes = state.cert_store.read().cert_hashes();

    let actor_ref: &Actor = &actor;
    Json(ConnectResponse {
        token: session_token,
        actor: actor_ref.clone(),
        wt_url,
        cert_hashes,
    })
    .into_response()
}

/// Check credentials in order: Tailscale → dev bypass → session token → password.
// w[auth.connect]
fn resolve_actor(
    state: &AppState,
    headers: &HeaderMap,
    body: &ConnectRequest,
) -> Option<Arc<Actor>> {
    // w[auth.tailscale]
    if state.trust_tailscale
        && let Some(actor) = tailscale::extract_actor(headers)
    {
        let session = uuid::Uuid::new_v4().to_string();
        return Some(Arc::new(Actor {
            session: Some(session),
            ..actor
        }));
    }

    // w[auth.dev]
    if state.dev_no_auth {
        return Some(Arc::new(Actor {
            kind: Some("dev".to_owned()),
            id: Some("dev".to_owned()),
            display: Some("dev".to_owned()),
            session: Some(uuid::Uuid::new_v4().to_string()),
        }));
    }

    // Bearer token from previous session.
    if let Some(token) = &body.token
        && let Some(actor) = password::verify_session_token(&state.sessions, token)
    {
        return Some(actor);
    }

    // Password login.
    // w[auth.password]
    if let (Some(pw), Some(hash)) = (&body.password, &state.password_hash)
        && password::verify_password(hash, pw)
    {
        return Some(Arc::new(Actor {
            kind: Some("password".to_owned()),
            id: Some("admin".to_owned()),
            display: Some("admin".to_owned()),
            session: Some(uuid::Uuid::new_v4().to_string()),
        }));
    }

    None
}
