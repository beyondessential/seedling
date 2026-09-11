use rcgen::{
    CertificateParams, CertificateSigningRequestParams, DistinguishedName, DnType, Issuer, KeyPair,
    PKCS_ECDSA_P256_SHA256,
};
use serde_json::json;

use crate::oi::test_support::TestOi;

fn self_signed(host: &str) -> (String, String) {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("keypair");
    let mut params = CertificateParams::new(vec![host.to_owned()]).expect("params");
    params.distinguished_name = DistinguishedName::new();
    let cert = params.self_signed(&key).expect("self-sign");
    (cert.pem(), key.serialize_pem())
}

/// Upsert a route53 provider and clear the catch-all `*` policy the first
/// provider automatically creates, so tests start from a clean policy set.
fn upsert_route53(oi: &TestOi, name: &str) {
    let outcome = oi
        .call(
            "/tls/dns-providers/upsert",
            json!({
                "name": name,
                "kind": "route53",
                "config": {
                    "access_key_id": "AKIA-TEST",
                    "secret_access_key": "secret",
                    "region": "us-east-1",
                },
            }),
        )
        .expect("provider upsert succeeds");
    if outcome["auto_policy_created"] == json!(true) {
        oi.call("/tls/policies/clear", json!({ "hostname": "*" }))
            .expect("auto policy clear succeeds");
    }
}

// i[verify tls.settings.get]
// i[verify tls.settings.set]
#[test]
fn settings_get_set_roundtrip() {
    let oi = TestOi::new();
    let initial = oi.call("/tls/settings/get", json!({})).unwrap();
    assert_eq!(initial["contact_email"], "");
    assert_eq!(initial["cert_profile"], json!(null));

    oi.call(
        "/tls/settings/set",
        json!({ "contact_email": "ops@example.com", "cert_profile": "shortlived" }),
    )
    .unwrap();
    let settings = oi.call("/tls/settings/get", json!({})).unwrap();
    assert_eq!(settings["contact_email"], "ops@example.com");
    assert_eq!(settings["cert_profile"], "shortlived");

    // Omitted fields are left unchanged; an explicit null clears the profile.
    oi.call("/tls/settings/set", json!({ "cert_profile": null }))
        .unwrap();
    let settings = oi.call("/tls/settings/get", json!({})).unwrap();
    assert_eq!(settings["contact_email"], "ops@example.com");
    assert_eq!(settings["cert_profile"], json!(null));
}

// i[verify tls.dns-provider.upsert]
// i[verify tls.dns-provider.list]
// i[verify tls.dns-provider.delete]
#[test]
fn dns_provider_upsert_list_delete_roundtrip() {
    let oi = TestOi::new();
    assert_eq!(
        oi.call("/tls/dns-providers/list", json!({})).unwrap()["providers"],
        json!([])
    );

    // r[verify tls.policy.auto-default]
    // The first provider auto-creates a catch-all `*` policy pointing at it.
    let outcome = oi
        .call(
            "/tls/dns-providers/upsert",
            json!({ "name": "aws-prod", "kind": "route53", "config": { "region": "us-east-1" } }),
        )
        .unwrap();
    assert_eq!(outcome["auto_policy_created"], true);
    let policies = oi.call("/tls/policies/list", json!({})).unwrap();
    assert_eq!(policies["policies"][0]["hostname"], "*");
    assert_eq!(policies["policies"][0]["dns_provider"], "aws-prod");
    oi.call("/tls/policies/clear", json!({ "hostname": "*" }))
        .unwrap();

    // A second upsert of the same name does not create another policy.
    let outcome = oi
        .call(
            "/tls/dns-providers/upsert",
            json!({ "name": "aws-prod", "kind": "route53", "config": { "region": "eu-west-1" } }),
        )
        .unwrap();
    assert_eq!(outcome["auto_policy_created"], false);

    let listed = oi.call("/tls/dns-providers/list", json!({})).unwrap();
    let providers = listed["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0]["name"], "aws-prod");
    assert_eq!(providers[0]["kind"], "route53");
    assert!(
        providers[0].get("config").is_none(),
        "credentials must not be listed: {providers:?}"
    );

    let (code, _) = oi
        .call(
            "/tls/dns-providers/upsert",
            json!({ "name": "bad", "kind": "cloudflare-nope", "config": {} }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");

    let (code, _) = oi
        .call(
            "/tls/dns-providers/upsert",
            json!({ "name": "  ", "kind": "route53", "config": {} }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");

    assert_eq!(
        oi.call("/tls/dns-providers/delete", json!({ "name": "aws-prod" }))
            .unwrap()["ok"],
        true
    );
    let (code, _) = oi
        .call("/tls/dns-providers/delete", json!({ "name": "aws-prod" }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify tls.policy.set-acme-dns]
// i[verify tls.policy.list]
// i[verify tls.policy.clear]
#[test]
fn policy_set_list_clear_roundtrip() {
    let oi = TestOi::new();
    upsert_route53(&oi, "aws-prod");

    let set = oi
        .call(
            "/tls/policies/set-acme-dns",
            json!({ "hostname": "*.example.com", "dns_provider": "aws-prod" }),
        )
        .unwrap();
    assert_eq!(set["ok"], true);
    assert_eq!(
        set["auto_issue_kicked"], false,
        "wildcard patterns have no concrete hostname to issue against"
    );

    let listed = oi.call("/tls/policies/list", json!({})).unwrap();
    let policies = listed["policies"].as_array().unwrap();
    assert_eq!(policies.len(), 1);
    assert_eq!(policies[0]["hostname"], "*.example.com");
    assert_eq!(policies[0]["strategy"], "acme_dns");
    assert_eq!(policies[0]["dns_provider"], "aws-prod");

    // The provider is now referenced; deleting it is refused.
    let (code, msg) = oi
        .call("/tls/dns-providers/delete", json!({ "name": "aws-prod" }))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("referenced"), "{msg}");

    assert_eq!(
        oi.call(
            "/tls/policies/clear",
            json!({ "hostname": "*.example.com" })
        )
        .unwrap()["ok"],
        true
    );
    let (code, _) = oi
        .call(
            "/tls/policies/clear",
            json!({ "hostname": "*.example.com" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");

    // With the policy gone the provider can be removed.
    assert_eq!(
        oi.call("/tls/dns-providers/delete", json!({ "name": "aws-prod" }))
            .unwrap()["ok"],
        true
    );
}

/// Sign a certificate over the public key inside `csr_pem`, carrying exactly
/// `sans`.
///
/// Stands in for a CA that signs a name set of its own choosing rather than
/// the one the CSR asked for — the case the runtime cannot control and must
/// not mistake for the requested name. The CSR's private key never leaves the
/// runtime, so the test signs over its public key just as a real CA does.
fn ca_signed_for_csr(
    csr_pem: &str,
    sans: &[&str],
    tweak: impl FnOnce(&mut CertificateParams),
) -> String {
    let csr = CertificateSigningRequestParams::from_pem(csr_pem).expect("parse csr");

    let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("ca keypair");
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("ca params");
    ca_params.distinguished_name = DistinguishedName::new();
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "Test CA");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let issuer = Issuer::new(ca_params, ca_key);

    let mut leaf = CertificateParams::new(sans.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
        .expect("leaf params");
    leaf.distinguished_name = DistinguishedName::new();
    tweak(&mut leaf);
    leaf.signed_by(&csr.public_key, &issuer)
        .expect("ca signs csr")
        .pem()
}

/// Begin a CSR for `hostname`, returning its row id and CSR PEM.
fn begin_csr(oi: &TestOi, hostname: &str) -> (i64, String) {
    let begun = oi
        .call(
            "/tls/certificates/csr/begin",
            json!({ "hostname": hostname }),
        )
        .expect("csr begin succeeds");
    (
        begun["id"].as_i64().unwrap(),
        begun["csr_pem"].as_str().unwrap().to_owned(),
    )
}

fn cert_row(oi: &TestOi, id: i64) -> serde_json::Value {
    let listed = oi.call("/tls/certificates/list", json!({})).unwrap();
    listed["certificates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == json!(id))
        .unwrap_or_else(|| panic!("certificate {id} is in the listing"))
        .clone()
}

fn warnings(response: &serde_json::Value) -> Vec<String> {
    response["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap().to_owned())
        .collect()
}

// i[verify tls.cert.csr.upload-cert]
// i[verify tls.cert.list]
// r[verify tls.cert.validation.san-coverage]
// r[verify tls.csr.flow]
#[test]
fn csr_upload_binds_to_the_issued_san_and_spares_the_requested_hostname() {
    let oi = TestOi::new();

    // A certificate is already serving www.example.com.
    let (incumbent_pem, incumbent_key) = self_signed("www.example.com");
    let incumbent = oi
        .call(
            "/tls/certificates/upload-manual",
            json!({ "cert_pem": incumbent_pem, "key_pem": incumbent_key }),
        )
        .unwrap();
    let incumbent_id = incumbent["id"].as_i64().unwrap();

    // An operator begins a CSR for the same hostname, and the CA signs for a
    // different name entirely.
    let (csr_id, csr_pem) = begin_csr(&oi, "www.example.com");
    let signed = ca_signed_for_csr(&csr_pem, &["example.com"], |_| {});
    let uploaded = oi
        .call(
            "/tls/certificates/csr/upload-cert",
            json!({ "id": csr_id, "cert_pem": signed }),
        )
        .expect("a certificate for the CSR's key is accepted");

    // Accepted, but bound to what it carries rather than what was asked for.
    assert_eq!(uploaded["primary_san"], "example.com");
    assert_eq!(uploaded["san_dns_names"], json!(["example.com"]));
    assert!(
        warnings(&uploaded).contains(&"request_not_covered".to_owned()),
        "{uploaded}"
    );

    let row = cert_row(&oi, csr_id);
    assert_eq!(row["state"], "active");
    assert_eq!(row["hostname"], "example.com");
    assert_eq!(row["requested_hostname"], "www.example.com");

    // The certificate that does cover www.example.com is untouched: a cert
    // that does not cover a hostname must not retire the one that does.
    let incumbent_row = cert_row(&oi, incumbent_id);
    assert_eq!(incumbent_row["state"], "active");
    assert_eq!(incumbent_row["hostname"], "www.example.com");
    assert_eq!(incumbent_row["requested_hostname"], json!(null));
}

// r[verify tls.cert.validation.san-coverage]
#[test]
fn csr_upload_accepts_a_wildcard_that_covers_the_request() {
    let oi = TestOi::new();

    let (id, csr_pem) = begin_csr(&oi, "foo.example.com");
    let signed = ca_signed_for_csr(&csr_pem, &["*.example.com"], |_| {});
    let uploaded = oi
        .call(
            "/tls/certificates/csr/upload-cert",
            json!({ "id": id, "cert_pem": signed }),
        )
        .unwrap();

    assert_eq!(uploaded["primary_san"], "*.example.com");
    assert!(
        !warnings(&uploaded).contains(&"request_not_covered".to_owned()),
        "a wildcard covering the requested name meets the request: {uploaded}"
    );
    assert_eq!(cert_row(&oi, id)["hostname"], "*.example.com");
}

// r[verify tls.cert.validation.san-coverage]
#[test]
fn csr_upload_warns_when_a_wildcard_is_one_label_short() {
    let oi = TestOi::new();

    // A wildcard covers exactly one extra left-most label, so *.example.com
    // does not reach a.b.example.com.
    let (id, csr_pem) = begin_csr(&oi, "a.b.example.com");
    let signed = ca_signed_for_csr(&csr_pem, &["*.example.com"], |_| {});
    let uploaded = oi
        .call(
            "/tls/certificates/csr/upload-cert",
            json!({ "id": id, "cert_pem": signed }),
        )
        .unwrap();

    assert!(
        warnings(&uploaded).contains(&"request_not_covered".to_owned()),
        "{uploaded}"
    );
}

// r[verify tls.cert.validation.san-coverage]
#[test]
fn csr_upload_supersedes_only_under_its_own_primary_san() {
    let oi = TestOi::new();

    let (old_pem, old_key) = self_signed("app.example.com");
    let old_id = oi
        .call(
            "/tls/certificates/upload-manual",
            json!({ "cert_pem": old_pem, "key_pem": old_key }),
        )
        .unwrap()["id"]
        .as_i64()
        .unwrap();

    let (new_id, csr_pem) = begin_csr(&oi, "app.example.com");
    let signed = ca_signed_for_csr(&csr_pem, &["app.example.com"], |_| {});
    let uploaded = oi
        .call(
            "/tls/certificates/csr/upload-cert",
            json!({ "id": new_id, "cert_pem": signed }),
        )
        .unwrap();

    assert!(
        !warnings(&uploaded).contains(&"request_not_covered".to_owned()),
        "{uploaded}"
    );
    assert_eq!(cert_row(&oi, old_id)["state"], "superseded");
    assert_eq!(cert_row(&oi, new_id)["state"], "active");
}

// i[verify tls.cert.csr.upload-cert]
#[test]
fn csr_upload_rejections_leave_the_row_and_the_incumbent_untouched() {
    let oi = TestOi::new();

    let (incumbent_pem, incumbent_key) = self_signed("www.example.com");
    let incumbent_id = oi
        .call(
            "/tls/certificates/upload-manual",
            json!({ "cert_pem": incumbent_pem, "key_pem": incumbent_key }),
        )
        .unwrap()["id"]
        .as_i64()
        .unwrap();

    let (id, csr_pem) = begin_csr(&oi, "www.example.com");

    // A certificate for somebody else's key.
    let (other_pem, _) = self_signed("www.example.com");
    let (code, _) = oi
        .call(
            "/tls/certificates/csr/upload-cert",
            json!({ "id": id, "cert_pem": other_pem }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");

    // A certificate for the right key that has already expired.
    let expired = ca_signed_for_csr(&csr_pem, &["www.example.com"], |p| {
        p.not_before = time::OffsetDateTime::from_unix_timestamp(1).unwrap();
        p.not_after = time::OffsetDateTime::from_unix_timestamp(2).unwrap();
    });
    let (code, _) = oi
        .call(
            "/tls/certificates/csr/upload-cert",
            json!({ "id": id, "cert_pem": expired }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");

    // Both the pending row and the serving certificate are as they were, so a
    // failed upload costs the operator nothing.
    let row = cert_row(&oi, id);
    assert_eq!(row["state"], "csr_pending");
    assert_eq!(row["requested_hostname"], "www.example.com");
    oi.call("/tls/certificates/csr/get", json!({ "id": id }))
        .expect("the CSR is still retrievable");
    assert_eq!(cert_row(&oi, incumbent_id)["state"], "active");
}

// i[verify tls.cert.upload-manual]
// i[verify tls.cert.list]
// i[verify tls.cert.delete]
#[test]
fn manual_certificate_upload_list_delete_roundtrip() {
    let oi = TestOi::new();
    assert_eq!(
        oi.call("/tls/certificates/list", json!({})).unwrap()["certificates"],
        json!([])
    );

    let (cert_pem, key_pem) = self_signed("foo.example.com");
    let uploaded = oi
        .call(
            "/tls/certificates/upload-manual",
            json!({ "cert_pem": cert_pem, "key_pem": key_pem, "note": "hand-issued" }),
        )
        .unwrap();
    assert_eq!(uploaded["primary_san"], "foo.example.com");
    assert!(
        uploaded["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("self_signed")),
        "{uploaded}"
    );
    let id = uploaded["id"].as_i64().unwrap();

    let listed = oi.call("/tls/certificates/list", json!({})).unwrap();
    let certs = listed["certificates"].as_array().unwrap();
    assert_eq!(certs.len(), 1);
    assert_eq!(certs[0]["id"], id);
    assert_eq!(certs[0]["hostname"], "foo.example.com");
    assert_eq!(certs[0]["state"], "active");
    assert_eq!(certs[0]["origin"], "manual");
    assert_eq!(certs[0]["self_signed"], true);
    assert_eq!(certs[0]["note"], "hand-issued");

    assert_eq!(
        oi.call("/tls/certificates/delete", json!({ "id": id }))
            .unwrap()["ok"],
        true
    );
    let (code, _) = oi
        .call("/tls/certificates/delete", json!({ "id": id }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify tls.cert.upload-manual]
#[test]
fn manual_certificate_upload_rejects_bad_input() {
    let oi = TestOi::new();

    let (code, _) = oi
        .call(
            "/tls/certificates/upload-manual",
            json!({ "cert_pem": "not a pem", "key_pem": "also not a pem" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");

    // Key that does not match the certificate's public key.
    let (cert_pem, _) = self_signed("foo.example.com");
    let (_, other_key) = self_signed("bar.example.com");
    let (code, _) = oi
        .call(
            "/tls/certificates/upload-manual",
            json!({ "cert_pem": cert_pem, "key_pem": other_key }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert_eq!(
        oi.call("/tls/certificates/list", json!({})).unwrap()["certificates"],
        json!([])
    );
}

// i[verify tls.cert.csr.begin]
// i[verify tls.cert.csr.get]
// i[verify tls.cert.csr.cancel]
#[test]
fn csr_begin_get_cancel_roundtrip() {
    let oi = TestOi::new();

    let begun = oi
        .call(
            "/tls/certificates/csr/begin",
            json!({ "hostname": "csr.example.com" }),
        )
        .unwrap();
    let id = begun["id"].as_i64().unwrap();
    let csr_pem = begun["csr_pem"].as_str().unwrap();
    assert!(csr_pem.contains("BEGIN CERTIFICATE REQUEST"), "{csr_pem}");

    let fetched = oi
        .call("/tls/certificates/csr/get", json!({ "id": id }))
        .unwrap();
    assert_eq!(fetched["csr_pem"], csr_pem);

    let listed = oi.call("/tls/certificates/list", json!({})).unwrap();
    let certs = listed["certificates"].as_array().unwrap();
    assert_eq!(certs[0]["state"], "csr_pending");
    assert_eq!(certs[0]["origin"], "csr");

    assert_eq!(
        oi.call("/tls/certificates/csr/cancel", json!({ "id": id }))
            .unwrap()["ok"],
        true
    );
    let (code, _) = oi
        .call("/tls/certificates/csr/get", json!({ "id": id }))
        .unwrap_err();
    assert_eq!(code, "not_found");
}

// i[verify tls.cert.csr.begin]
// i[verify tls.cert.csr.get]
// i[verify tls.cert.csr.upload-cert]
// i[verify tls.cert.csr.cancel]
#[test]
fn csr_flow_validation_errors() {
    let oi = TestOi::new();

    let (code, _) = oi
        .call("/tls/certificates/csr/begin", json!({ "hostname": "  " }))
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");

    let (code, msg) = oi
        .call(
            "/tls/certificates/csr/begin",
            json!({ "hostname": "csr.example.com", "key_type": "rsa4096" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("rsa4096"), "{msg}");

    let (code, _) = oi
        .call(
            "/tls/certificates/csr/upload-cert",
            json!({ "id": 999, "cert_pem": "whatever" }),
        )
        .unwrap_err();
    assert_eq!(code, "not_found");
    let (code, _) = oi
        .call("/tls/certificates/csr/cancel", json!({ "id": 999 }))
        .unwrap_err();
    assert_eq!(code, "not_found");

    // A pending CSR row rejects a garbage certificate and stays pending.
    let begun = oi
        .call(
            "/tls/certificates/csr/begin",
            json!({ "hostname": "csr.example.com" }),
        )
        .unwrap();
    let id = begun["id"].as_i64().unwrap();
    let (code, _) = oi
        .call(
            "/tls/certificates/csr/upload-cert",
            json!({ "id": id, "cert_pem": "not a pem" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    oi.call("/tls/certificates/csr/get", json!({ "id": id }))
        .unwrap();

    // Non-pending rows refuse the CSR sub-operations.
    let (cert_pem, key_pem) = self_signed("active.example.com");
    let uploaded = oi
        .call(
            "/tls/certificates/upload-manual",
            json!({ "cert_pem": cert_pem, "key_pem": key_pem }),
        )
        .unwrap();
    let active_id = uploaded["id"].as_i64().unwrap();
    for method in ["/tls/certificates/csr/get", "/tls/certificates/csr/cancel"] {
        let (code, msg) = oi.call(method, json!({ "id": active_id })).unwrap_err();
        assert_eq!(code, "requirements_invalid");
        assert!(msg.contains("active"), "{method}: {msg}");
    }
}

// i[verify tls.cert.issue-acme-dns]
#[test]
fn issue_acme_dns_fails_without_policy() {
    let oi = TestOi::new();
    let (code, msg) = oi
        .call(
            "/tls/certificates/issue-acme-dns",
            json!({ "hostname": "nopolicy.example.com" }),
        )
        .unwrap_err();
    assert_eq!(code, "internal");
    assert!(msg.contains("issuance failed"), "{msg}");
}

// i[verify tls.retry-block.set]
// i[verify tls.retry-block.list]
// i[verify tls.retry-block.clear]
#[test]
fn retry_block_set_list_clear_roundtrip() {
    let oi = TestOi::new();
    assert_eq!(
        oi.call("/tls/retry-blocks/list", json!({})).unwrap()["blocks"],
        json!([])
    );

    oi.call(
        "/tls/retry-blocks/set",
        json!({ "hostname": "stuck.example.com", "reason": "rate limited" }),
    )
    .unwrap();
    let listed = oi.call("/tls/retry-blocks/list", json!({})).unwrap();
    let blocks = listed["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["hostname"], "stuck.example.com");
    assert_eq!(blocks[0]["set_by"], "operator");
    assert_eq!(blocks[0]["reason"], "rate limited");

    let cleared = oi
        .call(
            "/tls/retry-blocks/clear",
            json!({ "hostname": "stuck.example.com" }),
        )
        .unwrap();
    assert_eq!(cleared["cleared"], true);
    let cleared = oi
        .call(
            "/tls/retry-blocks/clear",
            json!({ "hostname": "stuck.example.com" }),
        )
        .unwrap();
    assert_eq!(cleared["cleared"], false);
}

// i[verify tls.hostname.list]
// i[verify tls.cert.attempts.list]
#[test]
fn hostname_and_attempt_rollups_start_empty() {
    let oi = TestOi::new();
    assert_eq!(
        oi.call("/tls/hostnames/list", json!({})).unwrap()["hostnames"],
        json!([])
    );
    assert_eq!(
        oi.call("/tls/certificates/attempts/list", json!({}))
            .unwrap()["attempts"],
        json!([])
    );
}

// i[verify tls.policy.set-acme-dns]
// The FK refusal arrived as `not_found: db error: FOREIGN KEY constraint
// failed`, naming neither the provider nor the remedy, and handing a client
// the wrong error code to key off.
#[test]
fn set_acme_dns_names_an_unknown_provider() {
    let oi = TestOi::new();
    let (code, msg) = oi
        .call(
            "/tls/policies/set-acme-dns",
            json!({ "hostname": "a.example.com", "dns_provider": "aws-prd" }),
        )
        .unwrap_err();
    assert_eq!(code, "requirements_invalid");
    assert!(msg.contains("aws-prd"), "should name the provider: {msg}");
    assert!(
        !msg.contains("FOREIGN KEY"),
        "should not leak the constraint: {msg}"
    );
}

// r[verify tls.policy.wildcard]
// A pattern that matches nothing stored a policy row indistinguishable in the
// list from one that works.
#[test]
fn set_acme_dns_refuses_a_pattern_that_can_never_match() {
    let oi = TestOi::new();
    upsert_route53(&oi, "aws-prod");
    for bad in ["", "*.", "not a hostname", "-bad.example.com", "12345"] {
        let (code, _) = oi
            .call(
                "/tls/policies/set-acme-dns",
                json!({ "hostname": bad, "dns_provider": "aws-prod" }),
            )
            .unwrap_err();
        assert_eq!(code, "requirements_invalid", "should refuse {bad:?}");
    }
}

// r[verify tls.policy.wildcard]
#[test]
fn set_acme_dns_accepts_the_shapes_the_spec_defines() {
    let oi = TestOi::new();
    upsert_route53(&oi, "aws-prod");
    for good in ["*", "*.example.com", "host.example.com"] {
        oi.call(
            "/tls/policies/set-acme-dns",
            json!({ "hostname": good, "dns_provider": "aws-prod" }),
        )
        .unwrap_or_else(|e| panic!("should accept {good:?}: {e:?}"));
    }
}
