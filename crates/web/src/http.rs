use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::auth::{self, ConnectRequest};
use crate::spa;
use crate::state::AppState;

// Restricts scripts and plugin content to same-origin and denies framing.
// `style-src` allows inline styles because the UI toolkit (MUI/Emotion, xterm)
// injects `<style>` blocks at runtime; `connect-src` allows any `https:` origin
// because the SPA opens a WebTransport session to the WebTransport endpoint,
// which is a different (and operator-configurable) origin to this one.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
script-src 'self'; \
style-src 'self' 'unsafe-inline'; \
img-src 'self' data:; \
font-src 'self' data:; \
connect-src 'self' https:; \
object-src 'none'; \
base-uri 'self'; \
form-action 'self'; \
frame-ancestors 'none'";

// Denies access to browser features the interface does not use.
const PERMISSIONS_POLICY: &str = "accelerometer=(), autoplay=(), camera=(), \
display-capture=(), encrypted-media=(), fullscreen=(), geolocation=(), \
gyroscope=(), magnetometer=(), microphone=(), midi=(), payment=(), usb=()";

// w[transport.http]
pub fn router(state: AppState) -> Router {
    security_headers(
        Router::new()
            .route("/healthz", get(healthz))
            .route("/connect", post(connect))
            .fallback(spa::handler),
    )
    .with_state(state)
}

// w[transport.http.security-headers]
fn security_headers<S: Clone + Send + Sync + 'static>(router: Router<S>) -> Router<S> {
    let set_header = |name: HeaderName, value: &'static str| {
        SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
    };
    router
        .layer(set_header(
            header::CONTENT_SECURITY_POLICY,
            CONTENT_SECURITY_POLICY,
        ))
        .layer(set_header(header::X_FRAME_OPTIONS, "DENY"))
        .layer(set_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(set_header(
            HeaderName::from_static("permissions-policy"),
            PERMISSIONS_POLICY,
        ))
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

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt as _;

    use super::*;

    // w[verify transport.http.security-headers] Every response carries the
    // baseline security headers, and the CSP keeps the policies the SPA relies
    // on: inline styles, data: images/fonts, and WebTransport to an https origin
    // that is not 'self'.
    #[tokio::test]
    async fn responses_carry_security_headers() {
        let router: Router<()> = security_headers(Router::new().route("/", get(|| async { "ok" })));
        let res = router
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let headers = res.headers();
        assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert!(headers.contains_key("permissions-policy"));

        let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(csp.contains("object-src 'none'"));
        assert!(csp.contains("style-src 'self' 'unsafe-inline'"));
        assert!(csp.contains("img-src 'self' data:"));
        // WebTransport opens an https origin other than 'self'; the policy must
        // not regress to connect-src 'self'.
        assert!(csp.contains("connect-src 'self' https:"));
    }
}
