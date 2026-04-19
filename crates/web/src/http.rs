use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::auth::{self, ConnectRequest};
use crate::state::AppState;

// w[transport.http]
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/connect", post(connect))
        .fallback(spa_fallback)
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

// w[auth.connect]
async fn connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ConnectRequest>,
) -> impl IntoResponse {
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    auth::handle_connect(state, headers, host, body).await
}

async fn spa_fallback() -> impl IntoResponse {
    // w[spa.delivery] — SPA bundle will be embedded here in phase 3.
    (StatusCode::NOT_FOUND, "SPA not yet available")
}
