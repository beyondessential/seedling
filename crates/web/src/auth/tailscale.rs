use axum::http::HeaderMap;
use seedling_protocol::actor::Actor;

const HEADER_LOGIN: &str = "tailscale-user-login";
const HEADER_NAME: &str = "tailscale-user-name";

// w[auth.tailscale]
pub fn extract_actor(headers: &HeaderMap) -> Option<Actor> {
    let id = headers.get(HEADER_LOGIN)?.to_str().ok()?.to_owned();
    let display = headers
        .get(HEADER_NAME)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_else(|| id.clone());

    Some(Actor {
        kind: Some("tailscale".to_owned()),
        id: Some(id),
        display: Some(display),
        session: None,
    })
}
