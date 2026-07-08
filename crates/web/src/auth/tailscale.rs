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
        .map(decode_display_name)
        .unwrap_or_else(|| id.clone());

    Some(Actor {
        kind: Some("tailscale".to_owned()),
        id: Some(id),
        display: Some(display),
        session: None,
    })
}

/// Decode the display-name header. Tailscale Serve carries non-ASCII names as
/// MIME encoded-words (RFC 2047, e.g. `=?utf-8?q?F=C3=A9lix?=`) because HTTP
/// header values can't hold raw UTF-8. Plain ASCII names pass through unchanged;
/// anything that fails to decode falls back to the raw header value.
fn decode_display_name(raw: &str) -> String {
    rfc2047_decoder::decode(raw.as_bytes()).unwrap_or_else(|_| raw.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    // w[verify auth.tailscale]
    #[test]
    fn extract_with_login_and_name_uses_name_as_display() {
        let got = extract_actor(&headers(&[
            (HEADER_LOGIN, "alice@example.com"),
            (HEADER_NAME, "Alice Example"),
        ]))
        .expect("actor should be extracted");
        assert_eq!(got.kind.as_deref(), Some("tailscale"));
        assert_eq!(got.id.as_deref(), Some("alice@example.com"));
        assert_eq!(got.display.as_deref(), Some("Alice Example"));
        assert!(
            got.session.is_none(),
            "tailscale extractor leaves session unset"
        );
    }

    // w[verify auth.tailscale]
    #[test]
    fn decodes_rfc2047_encoded_display_name() {
        // Tailscale Serve carries non-ASCII names as MIME encoded-words.
        let got = extract_actor(&headers(&[
            (HEADER_LOGIN, "felix@example.com"),
            (HEADER_NAME, "=?utf-8?q?F=C3=A9lix_Saparelli?="),
        ]))
        .expect("actor should be extracted");
        assert_eq!(got.display.as_deref(), Some("Félix Saparelli"));
    }

    // w[verify auth.tailscale]
    #[test]
    fn extract_with_login_only_falls_back_to_login_as_display() {
        let got = extract_actor(&headers(&[(HEADER_LOGIN, "bob@example.com")]))
            .expect("actor should be extracted");
        assert_eq!(got.id.as_deref(), Some("bob@example.com"));
        assert_eq!(got.display.as_deref(), Some("bob@example.com"));
    }

    // w[verify auth.tailscale]
    #[test]
    fn extract_without_login_header_returns_none() {
        // Proxy did not inject the Tailscale headers — not a Tailscale request.
        assert!(extract_actor(&headers(&[(HEADER_NAME, "Alice")])).is_none());
        assert!(extract_actor(&headers(&[])).is_none());
    }

    // w[verify auth.tailscale]
    #[test]
    fn extract_with_non_ascii_login_header_returns_none() {
        // Non-ASCII header values cannot be represented as an Actor id; the
        // extractor must reject rather than panic.
        let mut h = HeaderMap::new();
        h.insert(
            HEADER_LOGIN,
            HeaderValue::from_bytes(b"\xff\xfe not-utf8").unwrap(),
        );
        assert!(extract_actor(&h).is_none());
    }
}
