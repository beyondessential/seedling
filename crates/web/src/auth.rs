use std::sync::Arc;

use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::json;

use seedling_protocol::actor::Actor;

pub mod password;
pub mod tailscale;

use crate::state::{self, AppState};

pub struct ConnectRequest {
    pub token: Option<String>,
    pub password: Option<String>,
}

impl ConnectRequest {
    /// Read a `/connect` body: a JSON object sent as `application/json`, whose
    /// `token` and `password` members are strings, and which carries at most
    /// one of them.
    // spec: WEB#authentication
    pub fn parse(content_type: Option<&str>, body: &[u8]) -> Result<Self, String> {
        let is_json = content_type
            .and_then(|ct| ct.split(';').next())
            .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("application/json"));
        if !is_json {
            return Err("the body must be sent as application/json".to_owned());
        }
        let serde_json::Value::Object(members) =
            serde_json::from_slice(body).map_err(|e| format!("the body is not JSON: {e}"))?
        else {
            return Err("the body must be a JSON object".to_owned());
        };
        let string_member = |name: &str| match members.get(name) {
            None => Ok(None),
            Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(format!("`{name}` must be a string")),
        };
        let token = string_member("token")?;
        let password = string_member("password")?;
        if token.is_some() && password.is_some() {
            return Err("send at most one of `token` and `password`".to_owned());
        }
        Ok(Self { token, password })
    }
}

#[derive(Serialize)]
pub struct ConnectResponse {
    pub token: String,
    pub actor: Actor,
    pub wt_url: String,
    pub cert_hashes: Vec<String>,
}

// spec: WEB#authentication
pub async fn handle_connect(
    state: AppState,
    headers: HeaderMap,
    host: Option<String>,
    body: ConnectRequest,
) -> Response {
    let maybe_actor = resolve_actor(&state, &headers, &body);

    let actor = match maybe_actor {
        Some(a) => a,
        None => return unauthorized(state.password_hash.as_deref()),
    };

    let session_token =
        password::issue_session_token(&state.sessions, Arc::clone(&actor), state.session_lifetime);

    let wt_token = state::issue_wt_token(&state.wt_tokens, Arc::clone(&actor));

    let wt_host = host.unwrap_or_else(|| crate::http::DEFAULT_HOSTNAME.to_owned());
    // Strip any port from the host header to reconstruct with wt_port.
    let wt_hostname = crate::http::hostname_from_host_header(&wt_host);
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

/// The `401` answer to a `/connect` no source authenticated: it names the
/// credential the client should collect, which is none when password login
/// could not authenticate any password.
// spec: WEB#authentication
fn unauthorized(password_hash: Option<&str>) -> Response {
    let required = if password::can_authenticate(password_hash) {
        "password"
    } else {
        "none"
    };
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "auth_required": required })),
    )
        .into_response()
}

/// Check credentials in order: Tailscale → dev bypass → session token → password.
// spec: WEB#authentication
fn resolve_actor(
    state: &AppState,
    headers: &HeaderMap,
    body: &ConnectRequest,
) -> Option<Arc<Actor>> {
    // spec: WEB#tailscale-identity
    if state.trust_tailscale
        && let Some(actor) = tailscale::extract_actor(headers)
    {
        let session = uuid::Uuid::new_v4().to_string();
        return Some(Arc::new(Actor {
            session: Some(session),
            ..actor
        }));
    }

    // spec: WEB#development-bypass
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
    // spec: WEB#password-login
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

#[cfg(test)]
mod tests {
    use argon2::password_hash::SaltString;
    use argon2::{Argon2, PasswordHasher};

    use super::*;

    // spec: WEB#authentication
    #[test]
    fn a_connect_body_is_a_json_object_of_strings_with_at_most_one_credential() {
        let parse = |ct: Option<&str>, body: &str| ConnectRequest::parse(ct, body.as_bytes());
        let json = Some("application/json");

        let empty = parse(json, "{}").unwrap();
        assert!(empty.token.is_none() && empty.password.is_none());
        let token = parse(json, r#"{"token":"abc"}"#).unwrap();
        assert_eq!(token.token.as_deref(), Some("abc"));
        let password = parse(
            Some("Application/JSON; charset=utf-8"),
            r#"{"password":"pw"}"#,
        )
        .unwrap();
        assert_eq!(password.password.as_deref(), Some("pw"));

        for (ct, body) in [
            (json, r#"{"token":"a","password":"b"}"#),
            (json, r#"["token"]"#),
            (json, "null"),
            (json, "not json"),
            (json, r#"{"token":1}"#),
            (json, r#"{"password":null}"#),
            (Some("text/plain"), "{}"),
            (None, "{}"),
        ] {
            assert!(parse(ct, body).is_err(), "{ct:?} {body}");
        }
    }

    async fn body_of(response: Response) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    // spec: WEB#authentication
    #[tokio::test]
    async fn unauthenticated_connect_asks_for_a_password_only_when_one_can_work() {
        let salt = SaltString::encode_b64(b"testsalt0123456").unwrap();
        let hash = Argon2::default()
            .hash_password(b"secret", &salt)
            .unwrap()
            .to_string();

        let (status, body) = body_of(unauthorized(Some(&hash))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, json!({ "auth_required": "password" }));

        for hash in [None, Some(""), Some("not a phc string")] {
            let (status, body) = body_of(unauthorized(hash)).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(body, json!({ "auth_required": "none" }), "{hash:?}");
        }
    }
}
