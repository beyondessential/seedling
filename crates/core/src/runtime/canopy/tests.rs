use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;

use super::*;
use crate::oi::test_support::TestOi;

fn provider(oi: &TestOi) -> Arc<CanopyProvider> {
    Arc::clone(oi.state.canopy_provider.as_ref().unwrap())
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap()
}

async fn encrypt_ticket(plaintext: &[u8], passphrase: &str) -> String {
    let pass =
        algae_cli::passphrases::Passphrase::new(secrecy::SecretString::from(passphrase.to_owned()));
    let mut ciphertext: Vec<u8> = Vec::new();
    algae_cli::streams::encrypt_stream(
        tokio::io::BufReader::new(plaintext),
        futures_util::io::Cursor::new(&mut ciphertext),
        Box::new(pass),
    )
    .await
    .unwrap();
    STANDARD.encode(&ciphertext)
}

// i[verify canopy.enrol]
#[test]
fn enrol_rejects_malformed_tickets() {
    let oi = TestOi::new();
    let provider = provider(&oi);
    let rt = runtime();

    let err = rt
        .block_on(provider.enrol("not base64 !!!", "pass"))
        .unwrap_err();
    assert!(matches!(err, CanopyError::InvalidTicket { .. }), "{err}");

    let not_age = STANDARD.encode(b"junk that is not an age ciphertext");
    let err = rt.block_on(provider.enrol(&not_age, "pass")).unwrap_err();
    assert!(matches!(err, CanopyError::InvalidTicket { .. }), "{err}");
}

// i[verify canopy.enrol]
#[test]
fn enrol_rejects_wrong_passphrase_and_bad_version() {
    let oi = TestOi::new();
    let provider = provider(&oi);
    let rt = runtime();

    let ticket_json = json!({
        "v": "enroll-999",
        "api_url": "https://canopy.example.com",
        "server_id": "5f2b0d3e-1111-2222-3333-444455556666",
        "token": "secret",
    })
    .to_string();
    let ticket = rt.block_on(encrypt_ticket(ticket_json.as_bytes(), "correct"));

    let err = rt.block_on(provider.enrol(&ticket, "wrong")).unwrap_err();
    assert!(matches!(err, CanopyError::Decrypt { .. }), "{err}");

    let err = rt.block_on(provider.enrol(&ticket, "correct")).unwrap_err();
    assert!(matches!(err, CanopyError::InvalidTicket { .. }), "{err}");
}

// i[verify canopy.enrol.single]
#[test]
fn enrol_refuses_while_registered() {
    let oi = TestOi::new();
    let provider = provider(&oi);
    *provider.registration.write() = Some(RegistrationInfo {
        server_id: "srv".into(),
        device_id: Some("dev".into()),
        api_url: "https://canopy.example.com/".into(),
    });

    let err = runtime()
        .block_on(provider.enrol("anything", "pass"))
        .unwrap_err();
    assert!(matches!(err, CanopyError::AlreadyEnrolled), "{err}");
}

// i[verify canopy.status]
#[test]
fn status_surfaces_registration_and_push_state() {
    let oi = TestOi::new();
    let provider = provider(&oi);
    *provider.registration.write() = Some(RegistrationInfo {
        server_id: "srv-1".into(),
        device_id: Some("dev-1".into()),
        api_url: "https://canopy.example.com/".into(),
    });
    *provider.push_status.write() = PushStatus {
        last_push_at: Some(Timestamp::now()),
        last_push_error: Some("boom".into()),
        last_response: Some(json!({ "backup_now": [] })),
    };

    let result = oi.call("/canopy/status", json!({})).unwrap();
    assert_eq!(result["enrolled"], json!(true));
    assert_eq!(result["server_id"], json!("srv-1"));
    assert_eq!(result["device_id"], json!("dev-1"));
    assert_eq!(result["api_url"], json!("https://canopy.example.com/"));
    assert!(result.get("last_push_at").is_some());
    assert_eq!(result["last_push_error"], json!("boom"));
    assert_eq!(result["last_response"], json!({ "backup_now": [] }));
}

// r[verify canopy.registration]
// i[verify canopy.deregister]
#[test]
fn deregister_removes_the_stored_registration() {
    let oi = TestOi::new();
    let provider = provider(&oi);
    let rt = runtime();

    let reg = Registration {
        server_id: Some("srv-1".into()),
        device_id: Some("dev-1".into()),
        api_url: Some("https://canopy.example.com/".into()),
        device_key: Some("pem".into()),
        ..Registration::default()
    };
    rt.block_on(registration::store_in(&provider.dir, &reg))
        .unwrap();
    // The stored registration survives on disk and round-trips.
    let loaded = rt
        .block_on(registration::load_from(&provider.dir))
        .unwrap()
        .unwrap();
    assert_eq!(loaded.server_id.as_deref(), Some("srv-1"));

    *provider.registration.write() = Some(RegistrationInfo {
        server_id: "srv-1".into(),
        device_id: Some("dev-1".into()),
        api_url: "https://canopy.example.com/".into(),
    });

    assert!(rt.block_on(provider.deregister()).unwrap());
    assert!(provider.registration_info().is_none());
    assert!(
        rt.block_on(registration::load_from(&provider.dir))
            .unwrap()
            .is_none()
    );
    // A second deregister has nothing to remove.
    assert!(!rt.block_on(provider.deregister()).unwrap());
}

// r[verify canopy.push]
#[test]
fn payload_identifies_seedling_and_covers_the_subsystems() {
    let oi = TestOi::new();
    let provider = provider(&oi);

    let payload = runtime().block_on(provider.build_payload());
    assert_eq!(payload["source"], json!("seedling"));
    assert_eq!(payload["seedlingVersion"], json!(env!("CARGO_PKG_VERSION")));
    assert!(payload["uptimeSecs"].is_u64());

    let health = payload["health"].as_array().unwrap();
    let names: Vec<&str> = health
        .iter()
        .map(|c| c["check"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["proxy", "resolver", "apps"]);
    // The stubbed container runtime reports no running containers.
    assert_eq!(health[0]["result"], json!("failed"));
    assert_eq!(health[1]["result"], json!("failed"));
    // No apps registered counts as healthy.
    assert_eq!(health[2]["result"], json!("passed"));
}
