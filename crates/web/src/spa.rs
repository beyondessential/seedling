use axum::body::Body;
use axum::http::{HeaderValue, Request, Response, StatusCode, header};
use axum::response::IntoResponse;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct Assets;

// w[spa.delivery]
pub async fn handler(req: Request<Body>) -> impl IntoResponse {
    let path = req.uri().path().trim_start_matches('/');

    let (path, is_asset) = if path.starts_with("assets/") {
        (path, true)
    } else {
        match Assets::get(path) {
            Some(_) => (path, false),
            None => ("index.html", false),
        }
    };

    let Some(file) = Assets::get(path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };

    let mime = mime_guess::from_path(path)
        .first_or_octet_stream()
        .to_string();

    let mut res = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &mime);

    if is_asset {
        res = res.header(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    } else {
        res = res.header(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        );
    }

    res.body(Body::from(file.data.into_owned()))
        .unwrap()
        .into_response()
}
