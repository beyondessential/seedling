use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, Request, Response, StatusCode, Uri, header};
use axum::response::IntoResponse;
use rust_embed::RustEmbed;

use crate::state::AppState;

#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct Assets;

// w[spa.delivery]
// w[routes.apps]
// w[impl routes.volumes]
pub async fn handler(State(state): State<AppState>, req: Request<Body>) -> impl IntoResponse {
    if let Some(port) = state.vite_port {
        return vite_proxy(port, req.uri()).await;
    }
    embedded(req.uri().path()).await
}

async fn embedded(path: &str) -> Response<Body> {
    let path = path.trim_start_matches('/');
    let is_asset = path.starts_with("assets/");

    let path = if !is_asset && Assets::get(path).is_none() {
        "index.html"
    } else {
        path
    };

    let Some(file) = Assets::get(path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };

    let mime = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();

    let cache = if is_asset {
        HeaderValue::from_static("public, max-age=31536000, immutable")
    } else {
        HeaderValue::from_static("no-cache, no-store, must-revalidate")
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, cache)
        .body(Body::from(file.data.into_owned()))
        .unwrap()
}

async fn vite_proxy(port: u16, uri: &Uri) -> Response<Body> {
    let url = format!("http://localhost:{port}{uri}");
    let res = match reqwest::get(&url).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("vite proxy: {e}");
            return (StatusCode::BAD_GATEWAY, "vite dev server unavailable").into_response();
        }
    };

    let status =
        StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let content_type = res.headers().get(header::CONTENT_TYPE).cloned();
    let body = res.bytes().await.unwrap_or_default();

    let mut builder = Response::builder().status(status);
    if let Some(ct) = content_type {
        builder = builder.header(header::CONTENT_TYPE, ct);
    }
    builder.body(Body::from(body)).unwrap()
}
