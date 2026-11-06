use std::collections::VecDeque;

use parking_lot::Mutex;
use rcgen::{CertificateSigningRequestParams, Issuer, KeyPair, PKCS_ECDSA_P256_SHA256};
use time::OffsetDateTime;

use super::*;
use crate::runtime::tls::{TlsCertOrigin, store};

const DAY: i64 = 86_400;

fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

fn response(state: &str, usable: bool) -> CertificateResponse {
    CertificateResponse::builder()
        .key_must_be_replaced(false)
        .name("a.example.com".to_owned())
        .revoked(false)
        .state(state.to_owned())
        .usable(usable)
        .build()
}

// r[verify tls.canopy.outcomes]
#[test]
fn a_usable_chain_is_issued() {
    let mut r = response("issued", true);
    r.chain = Some("PEM".to_owned());
    assert_eq!(
        classify_response(&r),
        Answer::Issued {
            chain_pem: "PEM".to_owned()
        }
    );
}

// r[verify tls.canopy.outcomes]
#[test]
fn a_renewal_under_way_still_delivers_the_chain_in_hand() {
    let mut r = response("pending", true);
    r.chain = Some("PEM".to_owned());
    assert!(matches!(classify_response(&r), Answer::Issued { .. }));
}

// r[verify tls.canopy.outcomes]
#[test]
fn a_pending_order_is_pending() {
    assert_eq!(
        classify_response(&response("pending", false)),
        Answer::Pending
    );
}

// r[verify tls.canopy.outcomes]
#[test]
fn a_failed_order_carries_cannopys_reason_and_is_not_a_refusal() {
    let mut r = response("failed", false);
    r.last_error = Some("dns challenge timed out".to_owned());
    assert_eq!(
        classify_response(&r),
        Answer::OrderFailed {
            reason: "dns challenge timed out".to_owned()
        }
    );
}

// r[verify tls.canopy.outcomes]
// r[verify tls.chain.refusal]
#[test]
fn a_state_the_runtime_does_not_know_is_neither_pending_nor_a_refusal() {
    match classify_response(&response("quarantined", false)) {
        Answer::Unavailable { reason } => assert!(reason.contains("quarantined"), "{reason}"),
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

// r[verify tls.canopy.revoked]
#[test]
fn revoked_and_condemned_keys_call_for_a_new_key() {
    let mut revoked = response("revoked", false);
    revoked.revoked = true;
    assert!(matches!(
        classify_response(&revoked),
        Answer::Revoked { .. }
    ));

    let mut condemned = response("issued", true);
    condemned.key_must_be_replaced = true;
    condemned.chain = Some("PEM".to_owned());
    assert!(
        matches!(classify_response(&condemned), Answer::Revoked { .. }),
        "a condemned key is not served even while the chain is usable"
    );
}

fn status(status: u16, slug: &str) -> RelayFailure {
    RelayFailure::Status {
        status,
        body: format!(
            r#"{{"type":"https://canopy.example/errors/{slug}","title":"t","status":{status},"detail":"because {slug}"}}"#
        ),
    }
}

// r[verify tls.canopy.outcomes]
// r[verify tls.chain.refusal]
#[test]
fn only_the_authoritys_own_no_is_a_refusal() {
    for failure in [
        status(403, "name-not-entitled"),
        status(403, "auth-insufficient-permissions"),
        status(409, "conflict"),
    ] {
        assert!(
            matches!(classify_failure(&failure), Answer::Refused { .. }),
            "{failure:?} is a refusal"
        );
    }
    assert!(
        matches!(
            classify_failure(&status(403, "name-not-entitled")),
            Answer::Refused { reason } if reason == "because name-not-entitled"
        ),
        "the reason is the one Canopy gave"
    );
}

// r[verify tls.canopy.outcomes]
// r[verify tls.chain.refusal]
#[test]
fn trouble_that_may_pass_is_not_a_refusal() {
    for failure in [
        status(409, "name-management-paused"),
        status(409, "something-new"),
        status(412, "device-has-no-server"),
        status(500, "database"),
        status(429, "rate-limited"),
        RelayFailure::Status {
            status: 409,
            body: "not json".to_owned(),
        },
        RelayFailure::Unreachable("no offer is live".to_owned()),
    ] {
        assert!(
            matches!(classify_failure(&failure), Answer::Unavailable { .. }),
            "{failure:?} is not a refusal"
        );
    }
}

// r[verify tls.canopy.outcomes]
#[test]
fn a_rejected_signing_request_is_the_runtimes_own_fault() {
    assert!(matches!(
        classify_failure(&status(400, "bad-request")),
        Answer::Malformed { .. }
    ));
}

// r[verify tls.canopy.revoked]
#[test]
fn a_condemned_key_calls_for_a_new_one() {
    assert!(matches!(
        classify_failure(&status(409, "certificate-key-compromised")),
        Answer::Revoked { .. }
    ));
}

#[derive(Clone)]
enum Reply {
    Pending,
    /// A chain for the key in the request, valid from `not_before` to
    /// `not_after`, in unix seconds.
    Issue {
        not_before: i64,
        not_after: i64,
    },
    /// A chain for some other key.
    IssueForOtherKey,
    Fail(RelayFailure),
    Revoked,
}

struct Fake {
    replies: Mutex<VecDeque<Reply>>,
    seen: Mutex<Vec<Seen>>,
    ca: Issuer<'static, KeyPair>,
}

#[derive(Clone, Debug)]
struct Seen {
    name: String,
    csr_der: Vec<u8>,
    app_type: Option<String>,
}

impl Fake {
    fn new(replies: impl IntoIterator<Item = Reply>) -> Self {
        let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "Fake Canopy CA");
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        Self {
            replies: Mutex::new(replies.into_iter().collect()),
            seen: Mutex::new(Vec::new()),
            ca: Issuer::new(params, ca_key),
        }
    }

    fn push(&self, reply: Reply) {
        self.replies.lock().push_back(reply);
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().clone()
    }

    fn sign(&self, csr_der: &[u8], not_before: i64, not_after: i64) -> String {
        let csr = rustls_pki_types::CertificateSigningRequestDer::from(csr_der.to_vec());
        let mut params = CertificateSigningRequestParams::from_der(&csr).unwrap();
        params.params.not_before = OffsetDateTime::from_unix_timestamp(not_before).unwrap();
        params.params.not_after = OffsetDateTime::from_unix_timestamp(not_after).unwrap();
        params.signed_by(&self.ca).unwrap().pem()
    }
}

impl CertificateRelay for &Fake {
    async fn request(
        &self,
        name: &str,
        csr_der_b64: &str,
        app_type: Option<&str>,
    ) -> std::result::Result<CertificateResponse, RelayFailure> {
        use base64::Engine as _;
        let csr_der = base64::engine::general_purpose::STANDARD
            .decode(csr_der_b64)
            .unwrap();
        self.seen.lock().push(Seen {
            name: name.to_owned(),
            csr_der: csr_der.clone(),
            app_type: app_type.map(str::to_owned),
        });
        let reply = self
            .replies
            .lock()
            .pop_front()
            .expect("the fake relay was asked more often than the test scripted");
        let base = CertificateResponse::builder()
            .key_must_be_replaced(false)
            .name(name.to_owned())
            .revoked(false);
        match reply {
            Reply::Pending => Ok(base.state("pending".to_owned()).usable(false).build()),
            Reply::Issue {
                not_before,
                not_after,
            } => Ok(base
                .state("issued".to_owned())
                .usable(true)
                .chain(self.sign(&csr_der, not_before, not_after))
                .build()),
            Reply::IssueForOtherKey => {
                let other = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
                let csr = keypair::build_csr(name, &other).unwrap();
                Ok(base
                    .state("issued".to_owned())
                    .usable(true)
                    .chain(self.sign(&csr.der, now() - DAY, now() + 90 * DAY))
                    .build())
            }
            Reply::Fail(failure) => Err(failure),
            Reply::Revoked => Ok(CertificateResponse::builder()
                .key_must_be_replaced(false)
                .name(name.to_owned())
                .revoked(true)
                .state("revoked".to_owned())
                .usable(false)
                .build()),
        }
    }
}

fn engine(fake: &Fake) -> (Engine<&Fake>, DbHandle) {
    let db = DbHandle::open_in_memory().unwrap();
    (
        Engine::new(db.clone(), Arc::new(Cipher::for_tests()), fake),
        db,
    )
}

fn ask(trigger: AttemptTrigger) -> Ask {
    Ask {
        hostname: "a.example.com".to_owned(),
        app_type: Some("tamanu".to_owned()),
        replacing: Identity::NONE,
        step: 0,
        since: now(),
        chain_len: 2,
        canopy_responsible: true,
        trigger,
    }
}

fn canopy_rows(db: &DbHandle) -> Vec<TlsCertificate> {
    db.call(|db| {
        store::list_certificates(db)
            .unwrap()
            .into_iter()
            .filter(|c| c.origin == TlsCertOrigin::Canopy)
            .collect()
    })
}

fn chain_row(db: &DbHandle) -> Option<TlsChainRow> {
    db.call(|db| store::get_chain_row(db, "a.example.com").unwrap())
}

fn attempts(db: &DbHandle) -> Vec<(AttemptOutcome, Option<i64>, Option<String>)> {
    db.call(|db| {
        store::list_attempts(db, Some("a.example.com"), 50)
            .unwrap()
            .into_iter()
            .map(|a| (a.outcome, a.cert_id, a.error))
            .collect()
    })
}

// r[verify tls.canopy.key]
// r[verify tls.canopy.request]
// r[verify tls.canopy.attempts]
#[tokio::test]
async fn the_first_request_stores_a_key_and_opens_one_order() {
    let fake = Fake::new([Reply::Pending, Reply::Pending]);
    let (engine, db) = engine(&fake);

    assert_eq!(
        engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap(),
        Outcome::Pending
    );
    let rows = canopy_rows(&db);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, TlsCertState::CsrPending);
    assert_eq!(rows[0].requested_hostname.as_deref(), Some("a.example.com"));
    assert_eq!(attempts(&db), vec![(AttemptOutcome::Pending, None, None)]);
    assert!(chain_row(&db).unwrap().row.open_order_since.is_some());

    // Asking again collects; it neither opens a second order nor adds a log entry.
    assert_eq!(
        engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap(),
        Outcome::Pending
    );
    assert_eq!(
        canopy_rows(&db).len(),
        1,
        "the same key is asked about again"
    );
    assert_eq!(attempts(&db).len(), 1);
    let seen = fake.requests();
    assert_eq!(seen[0].name, "a.example.com");
    assert_eq!(seen[0].app_type.as_deref(), Some("tamanu"));
}

// r[verify tls.canopy.collect]
// r[verify tls.canopy.serve]
// r[verify tls.canopy.attempts]
#[tokio::test]
async fn a_collected_chain_is_stored_and_served() {
    let n = now();
    let fake = Fake::new([
        Reply::Pending,
        Reply::Issue {
            not_before: n - DAY,
            not_after: n + 90 * DAY,
        },
    ]);
    let (engine, db) = engine(&fake);
    engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();

    let outcome = engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    let Outcome::Issued { cert_id } = outcome else {
        panic!("expected a certificate, got {outcome:?}");
    };

    let rows = canopy_rows(&db);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, cert_id);
    assert_eq!(rows[0].state, TlsCertState::Active);
    let served = db
        .call(|db| store::find_active_for_hostname(db, "a.example.com"))
        .unwrap()
        .expect("the certificate serves its hostname");
    assert_eq!(served.id, cert_id);
    assert_eq!(served.origin, TlsCertOrigin::Canopy);

    assert_eq!(
        attempts(&db),
        vec![(AttemptOutcome::Success, Some(cert_id), None)],
        "one order is one entry, closed when the certificate arrived"
    );
    assert!(chain_row(&db).is_none(), "the cycle is over");
}

// r[verify tls.canopy.attempts]
#[tokio::test]
async fn a_chain_on_the_first_request_is_still_one_logged_attempt() {
    let n = now();
    let fake = Fake::new([Reply::Issue {
        not_before: n - DAY,
        not_after: n + 90 * DAY,
    }]);
    let (engine, db) = engine(&fake);
    engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    assert_eq!(attempts(&db).len(), 1);
    assert_eq!(attempts(&db)[0].0, AttemptOutcome::Success);
}

fn renewing(db: &DbHandle) -> Ask {
    let cert = canopy_rows(db).remove(0);
    Ask {
        replacing: Identity {
            cert_id: Some(cert.id),
            not_after: cert.not_after,
        },
        trigger: AttemptTrigger::Renewal,
        ..ask(AttemptTrigger::Renewal)
    }
}

// r[verify tls.canopy.collect]
#[tokio::test]
async fn a_renewal_replaces_the_chain_under_the_same_key_once_it_lands() {
    let n = now();
    let first = Reply::Issue {
        not_before: n - 60 * DAY,
        not_after: n + 30 * DAY,
    };
    let fake = Fake::new([first.clone(), first]);
    let (engine, db) = engine(&fake);
    engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    let id = canopy_rows(&db)[0].id;

    // Canopy still holds the old chain: nothing newer has landed.
    assert_eq!(
        engine.ask(renewing(&db)).await.unwrap(),
        Outcome::Pending,
        "the chain in hand is not a renewal"
    );
    assert_eq!(canopy_rows(&db)[0].not_after, Some(n + 30 * DAY));
    let pending = attempts(&db);
    assert_eq!(
        pending[0].0,
        AttemptOutcome::Pending,
        "the renewal's order is open"
    );

    fake.push(Reply::Issue {
        not_before: n,
        not_after: n + 120 * DAY,
    });
    assert_eq!(
        engine.ask(renewing(&db)).await.unwrap(),
        Outcome::Issued { cert_id: id }
    );
    let rows = canopy_rows(&db);
    assert_eq!(rows.len(), 1, "same key, same row");
    assert_eq!(rows[0].not_after, Some(n + 120 * DAY));
    assert_eq!(attempts(&db)[0].0, AttemptOutcome::Success);
}

// r[verify tls.canopy.key]
#[tokio::test]
async fn a_chain_for_some_other_key_is_not_stored() {
    let fake = Fake::new([Reply::IssueForOtherKey]);
    let (engine, db) = engine(&fake);
    let outcome = engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    assert!(
        matches!(&outcome, Outcome::Failed(r) if r.contains("key")),
        "{outcome:?}"
    );
    assert_eq!(canopy_rows(&db)[0].state, TlsCertState::CsrPending);
}

// r[verify tls.chain.refusal]
// r[verify tls.canopy.attempts]
#[tokio::test]
async fn a_refusal_moves_a_hostname_with_a_certificate_on_at_once() {
    let n = now();
    let fake = Fake::new([Reply::Issue {
        not_before: n - 60 * DAY,
        not_after: n + 30 * DAY,
    }]);
    let (engine, db) = engine(&fake);
    engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    fake.push(Reply::Fail(status(403, "name-not-entitled")));

    let renewal = renewing(&db);
    let outcome = engine.ask(renewal.clone()).await.unwrap();
    assert!(matches!(outcome, Outcome::Refused(_)));

    let row = chain_row(&db).unwrap();
    assert_eq!(row.row.identity, renewal.replacing);
    assert_eq!(row.row.step, 1, "the next strategy is responsible");
    assert!(row.row.retry_not_before.is_some());
    assert_eq!(row.last_error.as_deref(), Some("because name-not-entitled"));
    let log = attempts(&db);
    assert_eq!(log[0].0, AttemptOutcome::Failure, "the refusal is logged");
}

// r[verify tls.chain.no-certificate]
#[tokio::test]
async fn a_refusal_does_not_move_a_hostname_with_nothing_to_replace() {
    let fake = Fake::new([Reply::Fail(status(403, "name-not-entitled"))]);
    let (engine, db) = engine(&fake);
    let outcome = engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    assert!(matches!(outcome, Outcome::Refused(_)));
    let row = chain_row(&db).unwrap();
    assert_eq!(row.row.step, 0, "new hostnames do not fall back");
    assert!(
        row.row.retry_not_before.is_some(),
        "but they are not asked every tick"
    );
}

// r[verify tls.chain.refusal]
#[tokio::test]
async fn trouble_reaching_canopy_is_retried_and_leaves_no_attempt() {
    let fake = Fake::new([Reply::Fail(RelayFailure::Unreachable(
        "relay down".to_owned(),
    ))]);
    let (engine, db) = engine(&fake);
    let outcome = engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    assert!(matches!(outcome, Outcome::Failed(_)));
    let row = chain_row(&db).unwrap();
    assert_eq!(row.row.step, 0);
    assert_eq!(
        row.row.retry_not_before, None,
        "asked again on the next tick"
    );
    assert!(row.last_error.unwrap().contains("relay down"));
    assert!(
        attempts(&db).is_empty(),
        "a question that was not put is not an attempt"
    );
}

// r[verify tls.canopy.outcomes]
#[tokio::test]
async fn a_rejected_signing_request_is_not_asked_again_at_once() {
    let fake = Fake::new([Reply::Fail(status(400, "bad-request"))]);
    let (engine, db) = engine(&fake);
    let outcome = engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    assert!(matches!(outcome, Outcome::Rejected(_)));
    assert!(chain_row(&db).unwrap().row.retry_not_before.is_some());
}

// r[verify tls.canopy.revoked]
#[tokio::test]
async fn a_revocation_replaces_the_key_and_opens_a_new_order() {
    let n = now();
    let fake = Fake::new([Reply::Issue {
        not_before: n - 60 * DAY,
        not_after: n + 30 * DAY,
    }]);
    let (engine, db) = engine(&fake);
    engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    let old = canopy_rows(&db).remove(0);
    fake.push(Reply::Revoked);
    fake.push(Reply::Pending);

    let outcome = engine.ask(renewing(&db)).await.unwrap();
    assert!(matches!(outcome, Outcome::Replaced(_)), "{outcome:?}");

    let rows = canopy_rows(&db);
    let old_now = rows.iter().find(|r| r.id == old.id).unwrap();
    assert_eq!(old_now.state, TlsCertState::Revoked);
    assert!(
        old_now.key_ciphertext.is_empty(),
        "the condemned key is destroyed"
    );
    let new = rows.iter().find(|r| r.id != old.id).expect("a new key");
    assert_eq!(new.state, TlsCertState::CsrPending);
    assert!(
        db.call(|db| store::find_active_for_hostname(db, "a.example.com"))
            .unwrap()
            .is_none(),
        "a revoked certificate is not served"
    );

    let row = chain_row(&db).unwrap();
    assert_eq!(row.row.identity.cert_id, Some(old.id));
    assert_eq!(
        row.row.step, 1,
        "the hostname passed to the next strategy at once"
    );
    assert!(row.row.open_order_since.is_some(), "the new order is open");
    let seen = fake.requests();
    assert_eq!(seen.len(), 3);
    assert_ne!(seen[0].csr_der, seen[2].csr_der);
}

// r[verify tls.canopy.revoked]
#[tokio::test]
async fn a_revocation_learnt_from_a_report_replaces_the_key_without_a_collection() {
    let n = now();
    let fake = Fake::new([
        Reply::Issue {
            not_before: n - 60 * DAY,
            not_after: n + 30 * DAY,
        },
        Reply::Pending,
    ]);
    let (engine, db) = engine(&fake);
    engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    let cert = canopy_rows(&db).remove(0);

    let spki = parse::leaf_spki_der(cert.cert_pem.as_deref().unwrap()).unwrap();
    let held = |fingerprint: String, revoked: bool| {
        bestool_canopy::schema::HeldCertificate::builder()
            .key_fingerprint(fingerprint)
            .key_must_be_replaced(false)
            .name("A.Example.com".to_owned())
            .revoked(revoked)
            .usable(!revoked)
            .build()
    };

    let condemned = db
        .call({
            let h = vec![
                held("00".repeat(32), true),
                held(key_fingerprint(&spki), false),
            ];
            move |db| condemned_by(db, &h)
        })
        .unwrap();
    assert!(
        condemned.is_empty(),
        "another key's revocation, and a held certificate in good standing"
    );

    let condemned = db
        .call({
            let h = vec![held(key_fingerprint(&spki), true)];
            move |db| condemned_by(db, &h)
        })
        .unwrap();
    assert_eq!(condemned.len(), 1);
    assert_eq!(condemned[0].cert_id, cert.id);

    engine
        .revoke_and_reorder(renewing(&db), cert.id, condemned[0].reason.clone())
        .await
        .unwrap();
    let rows = canopy_rows(&db);
    assert_eq!(
        rows.iter().find(|r| r.id == cert.id).unwrap().state,
        TlsCertState::Revoked
    );
    assert_eq!(
        rows.iter()
            .filter(|r| r.state == TlsCertState::CsrPending)
            .count(),
        1
    );
}

// r[verify tls.canopy.key]
// r[verify tls.chain.ladder]
#[tokio::test]
async fn a_restart_between_the_request_and_the_collection_finds_the_same_key_and_position() {
    let fake = Fake::new([
        Reply::Pending,
        Reply::Fail(status(403, "name-not-entitled")),
        Reply::Pending,
    ]);
    let db = DbHandle::open_in_memory().unwrap();
    let first = Engine::new(db.clone(), Arc::new(Cipher::for_tests()), &fake);
    first.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    let key_row = canopy_rows(&db)[0].id;
    drop(first);

    // Everything the first engine knew was in memory it no longer has.
    let second = Engine::new(db.clone(), Arc::new(Cipher::for_tests()), &fake);
    let outcome = second.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    assert!(matches!(outcome, Outcome::Refused(_)));
    assert_eq!(canopy_rows(&db).len(), 1, "no second key was made");
    assert_eq!(canopy_rows(&db)[0].id, key_row);
    let seen = fake.requests();
    assert_eq!(seen.len(), 2);

    // The refusal was recorded, and a third engine finds it where it was left.
    let third = Engine::new(db.clone(), Arc::new(Cipher::for_tests()), &fake);
    third.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    let row = chain_row(&db).unwrap();
    assert_eq!(
        row.last_error, None,
        "the later answer replaced the earlier reason"
    );
    assert_eq!(canopy_rows(&db).len(), 1);
}

// r[verify tls.cert.validation.san-coverage]
#[tokio::test]
async fn a_chain_naming_more_than_was_asked_for_is_not_stored() {
    struct Wide(Fake);
    impl CertificateRelay for &Wide {
        async fn request(
            &self,
            name: &str,
            csr_der_b64: &str,
            app_type: Option<&str>,
        ) -> std::result::Result<CertificateResponse, RelayFailure> {
            use base64::Engine as _;
            let der = base64::engine::general_purpose::STANDARD
                .decode(csr_der_b64)
                .unwrap();
            let csr = rustls_pki_types::CertificateSigningRequestDer::from(der);
            let mut params = CertificateSigningRequestParams::from_der(&csr).unwrap();
            params.params.subject_alt_names = vec![
                rcgen::SanType::DnsName(name.try_into().unwrap()),
                rcgen::SanType::DnsName("other.example.com".try_into().unwrap()),
            ];
            let _ = app_type;
            let pem = params.signed_by(&self.0.ca).unwrap().pem();
            Ok(CertificateResponse::builder()
                .key_must_be_replaced(false)
                .name(name.to_owned())
                .revoked(false)
                .state("issued".to_owned())
                .usable(true)
                .chain(pem)
                .build())
        }
    }

    let wide = Wide(Fake::new([]));
    let db = DbHandle::open_in_memory().unwrap();
    let engine = Engine::new(db.clone(), Arc::new(Cipher::for_tests()), &wide);
    let outcome = engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    assert!(
        matches!(&outcome, Outcome::Failed(r) if r.contains("other.example.com")),
        "{outcome:?}"
    );
    assert_eq!(canopy_rows(&db)[0].state, TlsCertState::CsrPending);
    assert!(
        db.call(|db| store::find_active_for_hostname(db, "other.example.com"))
            .unwrap()
            .is_none()
    );
}

// r[verify tls.canopy.attempts]
#[tokio::test]
async fn an_order_is_one_attempt_even_when_its_position_is_rewritten_under_it() {
    let fake = Fake::new([Reply::Pending, Reply::Pending]);
    let (engine, db) = engine(&fake);
    engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    assert_eq!(attempts(&db).len(), 1);

    // A position written without the open order, as a restarted cycle writes it.
    db.call(|db| {
        store::put_chain_row(
            db,
            "a.example.com",
            &chain::Row {
                identity: Identity::NONE,
                step: 0,
                since: 1,
                open_order_since: None,
                retry_not_before: None,
            },
            None,
        )
        .unwrap()
    });
    engine.ask(ask(AttemptTrigger::OnDemand)).await.unwrap();
    assert_eq!(
        attempts(&db).len(),
        1,
        "the open attempt is not opened twice"
    );
}
