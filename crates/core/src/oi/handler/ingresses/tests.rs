use serde_json::json;

use crate::oi::test_support::TestOi;

fn show(oi: &TestOi, name: &str) -> serde_json::Value {
    oi.call("/ingresses/site/show", json!({ "name": name }))
        .unwrap()
}

// spec: SITE#sources
#[test]
fn site_ingress_create_show_list_delete_roundtrip() {
    let oi = TestOi::new();
    let created = oi
        .call(
            "/ingresses/site/create",
            json!({
                "name": "legacy-portal",
                "hostname": "portal.example.com",
                "description": "old intranet",
            }),
        )
        .unwrap();
    assert_eq!(created["created"], true);

    let ing = show(&oi, "legacy-portal");
    assert_eq!(ing["hostname"], "portal.example.com");
    assert_eq!(ing["source"], "manual");
    assert_eq!(ing["tls_provider"], "acme");
    assert_eq!(ing["stale"], false);
    assert_eq!(ing["description"], "old intranet");
    assert_eq!(ing["attachments"], json!([]));

    let list = oi.call("/ingresses/site/list", json!({})).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);

    let deleted = oi
        .call("/ingresses/site/delete", json!({ "name": "legacy-portal" }))
        .unwrap();
    assert_eq!(deleted["deleted"], true);
    assert_eq!(
        oi.call("/ingresses/site/list", json!({})).unwrap(),
        json!([])
    );

    let (code, _) = oi
        .call("/ingresses/site/delete", json!({ "name": "legacy-portal" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
    let (code, _) = oi
        .call("/ingresses/site/show", json!({ "name": "legacy-portal" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// spec: SITE#sources
#[test]
fn site_ingress_create_rejects_invalid_hostname_and_tailscale_tls() {
    let oi = TestOi::new();

    let (code, msg) = oi
        .call(
            "/ingresses/site/create",
            json!({ "name": "wild", "hostname": "*.example.com" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("wildcard"), "{msg}");

    let (code, _) = oi
        .call(
            "/ingresses/site/create",
            json!({ "name": "dashes", "hostname": "-bad.example.com" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");

    let (code, _) = oi
        .call(
            "/ingresses/site/create",
            json!({ "name": "under", "hostname": "bad_host.example.com" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");

    let (code, msg) = oi
        .call(
            "/ingresses/site/create",
            json!({
                "name": "ts-only",
                "hostname": "ts.example.com",
                "tls_provider": "tailscale",
            }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("tailscale"), "{msg}");
}

// spec: SITE#sources
#[test]
fn site_ingress_update_description_and_tls_provider() {
    let oi = TestOi::new();
    oi.call(
        "/ingresses/site/create",
        json!({ "name": "portal", "hostname": "portal.example.com" }),
    )
    .unwrap();

    let updated = oi
        .call(
            "/ingresses/site/update",
            json!({ "name": "portal", "description": "front door", "tls_provider": "internal" }),
        )
        .unwrap();
    assert_eq!(updated["updated"], true);
    let ing = show(&oi, "portal");
    assert_eq!(ing["description"], "front door");
    assert_eq!(ing["tls_provider"], "internal");

    // An absent description leaves the existing value unchanged...
    oi.call(
        "/ingresses/site/update",
        json!({ "name": "portal", "tls_provider": "acme" }),
    )
    .unwrap();
    let ing = show(&oi, "portal");
    assert_eq!(ing["description"], "front door");
    assert_eq!(ing["tls_provider"], "acme");

    // ...while an explicit null clears it.
    oi.call(
        "/ingresses/site/update",
        json!({ "name": "portal", "description": null }),
    )
    .unwrap();
    let ing = show(&oi, "portal");
    assert_eq!(ing["description"], serde_json::Value::Null);
    assert_eq!(ing["tls_provider"], "acme");

    let (code, _) = oi
        .call(
            "/ingresses/site/update",
            json!({ "name": "portal", "tls_provider": "tailscale" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");

    let (code, _) = oi
        .call(
            "/ingresses/site/update",
            json!({ "name": "ghost", "tls_provider": "none" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// spec: SITE#attachments
#[test]
fn forward_attachment_attach_and_detach() {
    let oi = TestOi::new();
    oi.call(
        "/ingresses/site/create",
        json!({ "name": "app-front", "hostname": "app.example.com" }),
    )
    .unwrap();

    let attached = oi
        .call(
            "/ingresses/site/attach/forward",
            json!({
                "name": "app-front",
                "port": 443,
                "protocol": "http",
                "target_app": "my-app",
                "target_service": "web",
            }),
        )
        .unwrap();
    assert_eq!(attached["attached"], true);

    let ing = show(&oi, "app-front");
    let atts = ing["attachments"].as_array().unwrap();
    assert_eq!(atts.len(), 1);
    assert_eq!(atts[0]["port"], 443);
    assert_eq!(atts[0]["protocol"], "http");
    assert_eq!(atts[0]["target_kind"], "forward");
    assert_eq!(atts[0]["target_app"], "my-app");
    assert_eq!(atts[0]["target_service"], "web");

    let detached = oi
        .call(
            "/ingresses/site/detach",
            json!({ "name": "app-front", "port": 443, "protocol": "http" }),
        )
        .unwrap();
    assert_eq!(detached["detached"], true);
    assert_eq!(show(&oi, "app-front")["attachments"], json!([]));

    let (code, _) = oi
        .call(
            "/ingresses/site/detach",
            json!({ "name": "app-front", "port": 443, "protocol": "http" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");

    let (code, _) = oi
        .call(
            "/ingresses/site/attach/forward",
            json!({
                "name": "ghost",
                "port": 443,
                "protocol": "http",
                "target_app": "my-app",
                "target_service": "web",
            }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// spec: SITE#attachments
#[test]
fn redirect_attachment_defaults_and_validation() {
    let oi = TestOi::new();
    oi.call(
        "/ingresses/site/create",
        json!({ "name": "old-name", "hostname": "old.example.com" }),
    )
    .unwrap();

    oi.call(
        "/ingresses/site/attach/redirect",
        json!({
            "name": "old-name",
            "port": 443,
            "protocol": "http",
            "redirect_url": "https://new.example.com",
        }),
    )
    .unwrap();
    let ing = show(&oi, "old-name");
    let att = &ing["attachments"].as_array().unwrap()[0];
    assert_eq!(att["target_kind"], "redirect");
    assert_eq!(att["redirect_url"], "https://new.example.com");
    assert_eq!(att["redirect_code"], 307);
    assert_eq!(att["redirect_preserve_path"], true);

    let (code, msg) = oi
        .call(
            "/ingresses/site/attach/redirect",
            json!({
                "name": "old-name",
                "port": 8443,
                "protocol": "http",
                "redirect_url": "https://new.example.com",
                "redirect_code": 303,
            }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("redirect_code"), "{msg}");

    let (code, msg) = oi
        .call(
            "/ingresses/site/attach/redirect",
            json!({
                "name": "old-name",
                "port": 8443,
                "protocol": "http",
                "redirect_url": "ftp://new.example.com",
            }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("redirect_url"), "{msg}");

    let (code, msg) = oi
        .call(
            "/ingresses/site/attach/redirect",
            json!({
                "name": "old-name",
                "port": 8443,
                "protocol": "tcp",
                "redirect_url": "https://new.example.com",
            }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("protocol"), "{msg}");
}

// spec: SITE#tailscale-discovery
#[test]
fn discovery_status_and_refresh_without_tailscale() {
    let oi = TestOi::new();
    let status = oi
        .call("/ingresses/site/discovery/status", json!({}))
        .unwrap();
    assert_eq!(
        status["providers"],
        json!([]),
        "one object for each provider the runtime runs"
    );

    let (code, msg) = oi
        .call("/ingresses/site/discovery/refresh", json!({}))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("not configured"), "{msg}");
}

// spec: IDN#reserved-names
// The Tailscale provider disables whatever row holds this name, so a manual
// ingress created under it was permanently stale.
#[test]
fn reserved_site_ingress_name_is_rejected_at_creation() {
    let oi = TestOi::new();
    let (code, message) = oi
        .call(
            "/ingresses/site/create",
            json!({ "name": "tailscale", "hostname": "example.com" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(message.contains("reserved"), "message: {message}");
}

// spec: SITE#sources
// An attachment on port 0 is not routable; it used to be stored and then
// reach the proxy config rather than being refused with the field named.
#[test]
fn attaching_on_port_zero_is_refused() {
    let oi = TestOi::new();
    oi.call(
        "/ingresses/site/create",
        json!({ "name": "zero", "hostname": "zero.example.com" }),
    )
    .unwrap();

    let (code, msg) = oi
        .call(
            "/ingresses/site/attach/redirect",
            json!({
                "name": "zero",
                "port": 0,
                "protocol": "http",
                "redirect_url": "https://example.com/",
            }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(
        msg.contains("got 0"),
        "message should name the value: {msg}"
    );
}

fn event_types(
    rx: &mut tokio::sync::broadcast::Receiver<seedling_protocol::events::OiEvent>,
) -> Vec<serde_json::Value> {
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(serde_json::to_value(&e).unwrap());
    }
    events
}

fn create(oi: &TestOi, name: &str) {
    oi.call(
        "/ingresses/site/create",
        json!({ "name": name, "hostname": format!("{name}.example.com") }),
    )
    .unwrap();
}

fn attach_forward(
    oi: &TestOi,
    name: &str,
    service: &str,
) -> Result<serde_json::Value, (String, String)> {
    oi.call(
        "/ingresses/site/attach/forward",
        json!({
            "name": name,
            "port": 443,
            "protocol": "http",
            "target_app": "my-app",
            "target_service": service,
        }),
    )
}

fn record_discovered(oi: &TestOi, name: &str, stale: bool) {
    use crate::runtime::site_ingresses::{
        self, DiscoveryProvider, SiteIngressDef, SiteIngressSource, TlsProvider,
    };
    let def = SiteIngressDef {
        name: seedling_protocol::names::SiteIngressName::new(name).unwrap(),
        hostname: "host.ts.net".into(),
        description: None,
        source: SiteIngressSource::Discovered {
            provider: DiscoveryProvider::Tailscale,
            key: "n-1".into(),
        },
        tls_provider: TlsProvider::Tailscale,
        stale,
        created_at: jiff::Timestamp::now().to_string(),
    };
    oi.state
        .db
        .call(move |db| site_ingresses::create(db, &def))
        .unwrap();
}

// spec: INGM#creation
#[test]
fn creating_under_a_name_a_site_ingress_has_is_refused() {
    let oi = TestOi::new();
    create(&oi, "front");
    let (code, _) = oi
        .call(
            "/ingresses/site/create",
            json!({ "name": "front", "hostname": "other.example.com" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert_eq!(show(&oi, "front")["hostname"], "front.example.com");

    record_discovered(&oi, "found", false);
    let (code, _) = oi
        .call(
            "/ingresses/site/create",
            json!({ "name": "found", "hostname": "other.example.com" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid", "of either source");
}

// spec: INGM#update
#[test]
fn an_update_that_changes_nothing_emits_no_event() {
    let oi = TestOi::new();
    create(&oi, "front");
    let mut rx = oi.state.event_tx.subscribe();

    oi.call(
        "/ingresses/site/update",
        json!({ "name": "front", "tls_provider": "acme" }),
    )
    .unwrap();
    oi.call(
        "/ingresses/site/update",
        json!({ "name": "front", "description": null }),
    )
    .unwrap();
    assert!(event_types(&mut rx).is_empty());

    oi.call(
        "/ingresses/site/update",
        json!({ "name": "front", "description": "the front", "tls_provider": "none" }),
    )
    .unwrap();
    let events = event_types(&mut rx);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["type"], "SiteIngressUpdated");
    assert_eq!(events[0]["hostname"], "front.example.com");
    assert_eq!(events[0]["tls_provider"], "none");
    assert_eq!(events[0]["description"], "the front");
}

// spec: INGM#deletion
#[test]
fn deleting_emits_a_removal_for_each_attachment_and_the_real_source() {
    let oi = TestOi::new();
    create(&oi, "front");
    attach_forward(&oi, "front", "web").unwrap();
    oi.call(
        "/ingresses/site/attach/redirect",
        json!({
            "name": "front",
            "port": 80,
            "protocol": "http",
            "redirect_url": "https://front.example.com",
        }),
    )
    .unwrap();
    let mut rx = oi.state.event_tx.subscribe();

    oi.call("/ingresses/site/delete", json!({ "name": "front" }))
        .unwrap();
    let events = event_types(&mut rx);
    let types: Vec<_> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(
        types,
        [
            "SiteIngressAttachmentRemoved",
            "SiteIngressAttachmentRemoved",
            "SiteIngressDeleted"
        ]
    );
    assert_eq!(events[0]["port"], 80);
    assert_eq!(events[0]["target_kind"], "redirect");
    assert_eq!(events[1]["target_app"], "my-app");
    assert_eq!(events[2]["source"], "manual");
}

// spec: INGM#deletion
#[test]
fn a_discovered_site_ingress_can_be_deleted_once_its_provider_stops_reporting_it() {
    let oi = TestOi::new();
    record_discovered(&oi, "found", false);
    let (code, _) = oi
        .call("/ingresses/site/delete", json!({ "name": "found" }))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    show(&oi, "found");

    oi.state
        .db
        .call(|db| {
            crate::runtime::site_ingresses::set_stale(
                db,
                &seedling_protocol::names::SiteIngressName::new("found").unwrap(),
                true,
            )
        })
        .unwrap();
    let mut rx = oi.state.event_tx.subscribe();
    oi.call("/ingresses/site/delete", json!({ "name": "found" }))
        .unwrap();
    let events = event_types(&mut rx);
    assert_eq!(events[0]["type"], "SiteIngressDeleted");
    assert_eq!(events[0]["source"], "discovered");
}

// spec: INGM#attachments
#[test]
fn attaching_on_an_existing_port_replaces_the_target_and_keeps_created_at() {
    let oi = TestOi::new();
    create(&oi, "front");
    let mut rx = oi.state.event_tx.subscribe();

    attach_forward(&oi, "front", "web").unwrap();
    let created_at = show(&oi, "front")["attachments"][0]["created_at"].clone();
    let events = event_types(&mut rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], "SiteIngressAttachmentAdded");

    // The same target changes nothing and emits nothing.
    attach_forward(&oi, "front", "web").unwrap();
    assert!(event_types(&mut rx).is_empty());

    // Another target replaces it in place.
    attach_forward(&oi, "front", "api").unwrap();
    let atts = show(&oi, "front")["attachments"].clone();
    assert_eq!(atts.as_array().unwrap().len(), 1);
    assert_eq!(atts[0]["target_service"], "api");
    assert_eq!(
        atts[0]["created_at"], created_at,
        "decides ingress conflicts"
    );
    let events = event_types(&mut rx);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], "SiteIngressAttachmentUpdated");
    assert_eq!(events[0]["target_service"], "api");

    // A redirect on the same port replaces a forward.
    oi.call(
        "/ingresses/site/attach/redirect",
        json!({
            "name": "front",
            "port": 443,
            "protocol": "http",
            "redirect_url": "https://elsewhere.example.com",
            "redirect_preserve_path": false,
        }),
    )
    .unwrap();
    let att = &show(&oi, "front")["attachments"][0];
    assert_eq!(att["target_kind"], "redirect");
    assert_eq!(att["redirect_preserve_path"], false);
    assert!(att.get("target_app").is_none());
    let events = event_types(&mut rx);
    assert_eq!(events[0]["type"], "SiteIngressAttachmentUpdated");
    assert_eq!(events[0]["redirect_preserve_path"], false);
}

// spec: INGM#attachments
#[test]
fn detaching_reports_the_target_it_removed() {
    let oi = TestOi::new();
    create(&oi, "front");
    attach_forward(&oi, "front", "web").unwrap();
    let mut rx = oi.state.event_tx.subscribe();
    oi.call(
        "/ingresses/site/detach",
        json!({ "name": "front", "port": 443, "protocol": "http" }),
    )
    .unwrap();
    let events = event_types(&mut rx);
    assert_eq!(events[0]["type"], "SiteIngressAttachmentRemoved");
    assert_eq!(events[0]["target_kind"], "forward");
    assert_eq!(events[0]["target_app"], "my-app");
    assert_eq!(events[0]["target_service"], "web");
}

// spec: INGM#discovery-status
#[test]
fn discovery_health_is_null_before_the_first_poll() {
    let oi = TestOi::with_tailscale_provider();
    let status = oi
        .call("/ingresses/site/discovery/status", json!({}))
        .unwrap();
    let providers = status["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0]["name"], "tailscale");
    assert_eq!(providers[0]["healthy"], serde_json::Value::Null);
    assert!(providers[0].get("last_poll_at").is_none());
    assert!(providers[0].get("last_error").is_none());

    oi.call("/ingresses/site/discovery/refresh", json!({}))
        .unwrap();
}
