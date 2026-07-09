use serde_json::json;

use crate::oi::test_support::TestOi;

fn show(oi: &TestOi, name: &str) -> serde_json::Value {
    oi.call("/ingresses/site/show", json!({ "name": name }))
        .unwrap()
}

// r[verify ingress.site.lifecycle]
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

// r[verify ingress.site.lifecycle]
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

// r[verify ingress.site.lifecycle]
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

// r[verify ingress.site.attachment]
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

// r[verify ingress.site.attachment]
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

// r[verify ingress.site.tailscale]
#[test]
fn discovery_status_and_refresh_without_tailscale() {
    let oi = TestOi::new();
    let status = oi
        .call("/ingresses/site/discovery/status", json!({}))
        .unwrap();
    let providers = status["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0]["name"], "tailscale");
    assert_eq!(providers[0]["healthy"], false);
    assert_eq!(providers[0]["ingresses"], json!([]));
    assert!(
        providers[0]["last_error"]
            .as_str()
            .unwrap()
            .contains("not configured")
    );

    let (code, msg) = oi
        .call("/ingresses/site/discovery/refresh", json!({}))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("not configured"), "{msg}");
}
