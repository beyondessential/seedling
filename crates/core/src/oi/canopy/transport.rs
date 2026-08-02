//! The typed Canopy client's view of the relay.
//!
//! Everything above this — the generated wire types, the per-endpoint methods,
//! gzipping, status handling — is transport-agnostic, so implementing one trait
//! gives Seedling the whole typed Canopy interface without a Canopy identity of
//! its own.

use std::sync::Arc;

use bestool_canopy::{CanopyRequest, CanopyResponse, CanopyTransport, async_trait};
use bytes::Bytes;
use miette::{IntoDiagnostic as _, Result, WrapErr as _, miette};
use seedling_protocol::canopy::Headers;

use super::{CanopyState, RelayFailure, relay_request};

/// A [`CanopyTransport`] that carries each call over the operator interface to
/// a client that has offered to issue it.
pub struct OiCanopyTransport {
    canopy: Arc<CanopyState>,
}

impl OiCanopyTransport {
    pub fn new(canopy: Arc<CanopyState>) -> Self {
        Self { canopy }
    }
}

#[async_trait]
impl CanopyTransport for OiCanopyTransport {
    // i[canopy.unavailable]
    async fn call(&self, request: CanopyRequest) -> Result<CanopyResponse> {
        if !self.canopy.is_enabled() {
            return Err(miette!("Canopy access is turned off for this instance"));
        }
        // Nothing is queued for a provider that might turn up later: a caller
        // that wants to retry is better placed to decide when than the relay is.
        let offer = self
            .canopy
            .current()
            .ok_or_else(|| miette!("no client is currently offering to reach Canopy"))?;

        // i[canopy.relay.limits]
        let _slot = self.canopy.acquire_slot().await;

        let (parts, body) = request.into_parts();
        let path = parts
            .uri
            .path_and_query()
            .map(|pq| pq.as_str().to_owned())
            .unwrap_or_else(|| parts.uri.path().to_owned());
        let headers = to_headers(&parts.headers)
            .wrap_err("preparing the request headers for the Canopy relay")?;

        let response = relay_request(&offer, parts.method.as_str(), &path, headers, &body)
            .await
            .map_err(|e| match e {
                // A client error already names Canopy; the others describe the
                // relay itself, so say which of the two failed.
                RelayFailure::Client(_) => miette!("{e}"),
                other => miette!("relaying {} {path} failed: {other}", parts.method),
            })?;

        let mut built = http::Response::builder().status(response.status);
        for (name, value) in &response.headers {
            built = built.header(name, value);
        }
        built
            .body(Bytes::from(response.body))
            .into_diagnostic()
            .wrap_err("building the Canopy response")
    }
}

/// Flatten an [`http::HeaderMap`] to the wire's name-to-value map.
///
/// A name that appears more than once has its values combined in order,
/// separated by a comma and a space, per RFC 9110's field-line combination
/// rules. `Set-Cookie` is exempt from those rules and so is not faithfully
/// represented; the relay is not intended to carry it.
fn to_headers(map: &http::HeaderMap) -> Result<Headers> {
    let mut out = Headers::new();
    for (name, value) in map {
        // A header value may hold bytes that are not text. Rather than dropping
        // it or mangling it into something that is not what the peer sent, say
        // so: a Canopy call has no business carrying one.
        let value = value.to_str().into_diagnostic().wrap_err_with(|| {
            format!("header {name} holds a value that is not text and cannot be relayed")
        })?;
        out.entry(name.as_str().to_ascii_lowercase())
            .and_modify(|existing| {
                existing.push_str(", ");
                existing.push_str(value);
            })
            .or_insert_with(|| value.to_owned());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use seedling_protocol::canopy::{MAX_RELAY_BODY, RelayResponse, read_frame, write_frame};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, DuplexStream};

    use super::super::peer::DuplexPeer;
    use super::*;

    /// What the transport put on the wire: method, path, headers, body.
    type SentRequest = (String, String, Headers, Vec<u8>);

    /// Answer every relayed request with `status` and `body`, recording the
    /// request the transport produced.
    fn transport_answering(
        status: u16,
        body: &'static [u8],
    ) -> (
        OiCanopyTransport,
        Arc<CanopyState>,
        std::sync::mpsc::Receiver<SentRequest>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        let state = Arc::new(CanopyState::new());
        state.offer(
            0,
            "test".into(),
            "https://example.invalid".into(),
            None,
            DuplexPeer::new(move |mut stream: DuplexStream| {
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut all = Vec::new();
                    stream.read_to_end(&mut all).await.unwrap();
                    let (req, sent): (seedling_protocol::canopy::RelayRequest, Vec<u8>) =
                        read_frame(&all[..], MAX_RELAY_BODY).await.unwrap();
                    tx.send((req.method, req.path, req.headers, sent)).unwrap();

                    write_frame(
                        &mut stream,
                        &RelayResponse::Response {
                            status,
                            headers: Headers::from([(
                                "content-type".to_owned(),
                                "application/json".to_owned(),
                            )]),
                        },
                        body,
                    )
                    .await
                    .unwrap();
                    stream.shutdown().await.unwrap();
                });
            }),
        );
        (OiCanopyTransport::new(Arc::clone(&state)), state, rx)
    }

    // i[verify canopy.relay]
    #[tokio::test]
    async fn a_typed_call_crosses_the_relay_and_comes_back() {
        let (transport, _state, rx) = transport_answering(200, br#"{"ok":true}"#);

        let request = http::Request::builder()
            .method("POST")
            .uri("/status/abc?force=1")
            .header("content-type", "application/json")
            .header("content-encoding", "gzip")
            .body(Bytes::from_static(b"gzipped"))
            .unwrap();
        let response = transport.call(request).await.expect("a response");

        assert_eq!(response.status(), 200);
        assert_eq!(response.body().as_ref(), br#"{"ok":true}"#);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "application/json"
        );

        let (method, path, headers, body) = rx.recv().unwrap();
        assert_eq!(method, "POST");
        assert_eq!(path, "/status/abc?force=1", "the query rides along");
        assert_eq!(headers.get("content-encoding").unwrap(), "gzip");
        assert_eq!(body, b"gzipped");
    }

    // i[verify canopy.relay]
    #[tokio::test]
    async fn a_non_success_status_reaches_the_caller_rather_than_erroring() {
        // The client gives specific codes meaning — a backup-target 412 means a
        // dormant device — so the transport must not turn them into errors.
        let (transport, _state, _rx) = transport_answering(412, b"dormant");

        let request = http::Request::builder()
            .method("GET")
            .uri("/backup-target")
            .body(Bytes::new())
            .unwrap();
        let response = transport.call(request).await.expect("412 is a response");
        assert_eq!(response.status(), 412);
        assert_eq!(response.body().as_ref(), b"dormant");
    }

    // i[verify canopy.unavailable]
    #[tokio::test]
    async fn a_call_with_no_offer_fails_at_once() {
        let state = Arc::new(CanopyState::new());
        let transport = OiCanopyTransport::new(state);

        let request = http::Request::builder()
            .uri("/servers/self")
            .body(Bytes::new())
            .unwrap();
        let err = transport
            .call(request)
            .await
            .expect_err("nothing to relay to");
        assert!(
            err.to_string().contains("no client"),
            "unhelpful error: {err}"
        );
    }

    // i[verify canopy.unavailable]
    #[tokio::test]
    async fn a_call_while_disabled_says_so_rather_than_blaming_the_client() {
        let (transport, state, _rx) = transport_answering(200, b"{}");
        state.set_enabled(false);

        let request = http::Request::builder()
            .uri("/servers/self")
            .body(Bytes::new())
            .unwrap();
        let err = transport.call(request).await.expect_err("disabled");
        assert!(err.to_string().contains("turned off"), "{err}");
    }

    // i[verify canopy.relay]
    #[test]
    fn repeated_headers_are_combined_in_order() {
        let mut map = http::HeaderMap::new();
        map.append("accept", "text/plain".parse().unwrap());
        map.append("accept", "application/json".parse().unwrap());
        map.append("X-Single", "one".parse().unwrap());

        let headers = to_headers(&map).unwrap();
        assert_eq!(
            headers.get("accept").unwrap(),
            "text/plain, application/json"
        );
        assert_eq!(
            headers.get("x-single").unwrap(),
            "one",
            "names are lower-cased"
        );
    }

    // i[verify canopy.relay]
    #[test]
    fn a_header_value_that_is_not_text_is_refused_rather_than_mangled() {
        let mut map = http::HeaderMap::new();
        map.append(
            "x-binary",
            http::HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap(),
        );
        let err = to_headers(&map).expect_err("not representable on the wire");
        assert!(err.to_string().contains("x-binary"), "{err}");
    }
}
