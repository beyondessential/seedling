use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::auth::{self, ConnectRequest};
use crate::spa;
use crate::state::AppState;

// Restricts scripts and plugin content to same-origin and denies framing.
// `style-src` allows inline styles because the UI toolkit (MUI/Emotion, xterm)
// injects `<style>` blocks at runtime. `connect-src` is completed per-request
// (see `content_security_policy`) to name the exact WebTransport origin.
const CSP_BASE: &str = "default-src 'self'; \
script-src 'self'; \
style-src 'self' 'unsafe-inline'; \
img-src 'self' data:; \
font-src 'self' data:; \
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
    let wt_port = state.wt_port;
    security_headers(
        Router::new()
            .route("/healthz", get(healthz))
            .route("/connect", post(connect))
            .fallback(spa::handler),
        wt_port,
    )
    .with_state(state)
}

// w[transport.http.security-headers]
fn security_headers<S: Clone + Send + Sync + 'static>(
    router: Router<S>,
    wt_port: u16,
) -> Router<S> {
    let set_header = |name: HeaderName, value: &'static str| {
        SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
    };
    router
        .layer(middleware::from_fn_with_state(
            wt_port,
            content_security_policy,
        ))
        .layer(set_header(header::X_FRAME_OPTIONS, "DENY"))
        .layer(set_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(set_header(
            HeaderName::from_static("permissions-policy"),
            PERMISSIONS_POLICY,
        ))
        // Cross-origin isolation: the console embeds no third-party content, so
        // require every embedded resource to be same-origin or explicitly opt
        // in, and sever the opener relationship with any cross-origin window.
        .layer(set_header(
            HeaderName::from_static("cross-origin-embedder-policy"),
            "require-corp",
        ))
        .layer(set_header(
            HeaderName::from_static("cross-origin-opener-policy"),
            "same-origin",
        ))
        .layer(set_header(
            HeaderName::from_static("cross-origin-resource-policy"),
            "same-origin",
        ))
}

// w[transport.http.security-headers]
// connect-src names the WebTransport origin precisely instead of a `https:`
// scheme wildcard. The endpoint shares the page's hostname (the WebTransport
// URL is built the same way in `auth::handle_connect`) on the dedicated
// `wt_port`, so reconstruct it from the request Host header.
async fn content_security_policy(State(wt_port): State<u16>, req: Request, next: Next) -> Response {
    let connect_src = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|host| host.split(':').next().unwrap_or(host))
        .map(|hostname| format!("connect-src 'self' https://{hostname}:{wt_port}"))
        .unwrap_or_else(|| "connect-src 'self'".to_owned());

    let mut res = next.run(req).await;
    if let Ok(value) = HeaderValue::from_str(&format!("{CSP_BASE}; {connect_src}")) {
        res.headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, value);
    }
    res
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
    use tower::ServiceExt as _;

    use super::*;

    // w[verify transport.http.security-headers] Every response carries the
    // baseline security headers, the cross-origin isolation headers, and a CSP
    // that keeps the policies the SPA relies on (inline styles, data:
    // images/fonts) while naming the WebTransport origin precisely rather than
    // with a scheme wildcard.
    #[tokio::test]
    async fn responses_carry_security_headers() {
        let router: Router<()> =
            security_headers(Router::new().route("/", get(|| async { "ok" })), 8443);
        let res = router
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::HOST, "console.example:7890")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let headers = res.headers();
        assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert!(headers.contains_key("permissions-policy"));
        assert_eq!(headers["cross-origin-embedder-policy"], "require-corp");
        assert_eq!(headers["cross-origin-opener-policy"], "same-origin");

        let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(csp.contains("object-src 'none'"));
        assert!(csp.contains("style-src 'self' 'unsafe-inline'"));
        assert!(csp.contains("img-src 'self' data:"));
        // connect-src must name the WebTransport origin (host without its port,
        // plus wt_port) and must not regress to a scheme wildcard.
        assert!(csp.contains("connect-src 'self' https://console.example:8443"));
        assert!(!csp.contains("https:;") && !csp.ends_with("https:"));
    }
}
