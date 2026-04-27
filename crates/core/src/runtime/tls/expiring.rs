//! Periodic sweep that files / clears `cert_expiring_soon` faults.
//!
//! The runtime cannot autonomously renew manual or CSR-derived certs (no
//! key + CA pairing it owns). To give operators warning before such a cert
//! expires, we walk the current TLS-terminating ingresses, look up the
//! cert covering each hostname, and file a `cert_expiring_soon` fault per
//! ingress when the cert is within fourteen days of `not_after`.
//!
//! ACME-DNS-issued certs are exempt: the renewal task plus the
//! manual-near-expiry-via-acme-dns upgrade path handle them autonomously.
//!
//! Faults are cleared on the same sweep when (a) the cert covering an
//! affected ingress is no longer expiring, (b) the cert's origin has
//! transitioned to acme_dns (the runtime took over renewal), or (c) the
//! ingress no longer exists.

use jiff::Timestamp;
use seedling_protocol::names::AppName;

use super::{TlsCertOrigin, state};
use crate::runtime::{db::Db, faults};

/// Window before `not_after` where the runtime starts surfacing the
/// fault. Matches the spec's fourteen-day promise.
pub const EXPIRY_WINDOW_SECS: i64 = 14 * 24 * 60 * 60;

const FAULT_KIND: &str = "cert_expiring_soon";

/// One TLS-terminating ingress declared by an app, plus the hostname it
/// claims. The sweep takes a slice of these so callers (notably the
/// reconciler) can build the list once from the apps snapshot they
/// already have, rather than re-walking the registry.
#[derive(Debug, Clone)]
pub struct IngressTarget {
    pub app: AppName,
    pub ingress_name: String,
    pub hostname: String,
}

/// Walk `targets` and reconcile `cert_expiring_soon` faults against them.
/// Pure DB work; safe to call from inside a `DbHandle::call` closure.
// r[impl tls.fault.expiring]
pub fn sweep(db: &Db, targets: &[IngressTarget]) -> rusqlite::Result<()> {
    let snap = state::Snapshot::load(db)?;
    let now = Timestamp::now().as_second();

    // Compute the set of (app, ingress, hostname, description) tuples
    // that should currently carry a fault.
    let mut desired: Vec<DesiredFault> = Vec::new();
    for t in targets {
        if let Some(d) = compute_desired(&snap, t, now) {
            desired.push(d);
        }
    }

    // Reconcile against currently-active cert_expiring_soon faults.
    let active = faults::list_active_faults(db, None)?;
    let active: Vec<&faults::FaultRecord> =
        active.iter().filter(|f| f.kind == FAULT_KIND).collect();

    // Clear faults whose target no longer needs one.
    for f in &active {
        let still_wanted = desired.iter().any(|d| d.matches(f));
        if !still_wanted {
            faults::clear_fault(db, &f.id, &f.app)?;
        }
    }

    // File faults that aren't yet present.
    for d in &desired {
        let already = active.iter().any(|f| d.matches(f));
        if already {
            continue;
        }
        faults::file_fault(
            db,
            &d.app,
            Some("ingress"),
            Some(&d.ingress_name),
            None,
            FAULT_KIND,
            &d.description,
        )?;
    }

    Ok(())
}

#[derive(Debug)]
struct DesiredFault {
    app: AppName,
    ingress_name: String,
    description: String,
}

impl DesiredFault {
    fn matches(&self, record: &faults::FaultRecord) -> bool {
        record.kind == FAULT_KIND
            && record.app == self.app
            && record.resource_name.as_deref() == Some(self.ingress_name.as_str())
    }
}

fn compute_desired(
    snap: &state::Snapshot,
    target: &IngressTarget,
    now: i64,
) -> Option<DesiredFault> {
    let st = state::compute_state(snap, &target.hostname);
    let cert = st.active_cert?;
    // Only manual / CSR-derived certs warrant the fault. ACME-DNS certs
    // are renewed autonomously; the operator does not need to be paged.
    if !matches!(cert.origin, TlsCertOrigin::Manual | TlsCertOrigin::Csr) {
        return None;
    }
    let not_after = cert.not_after?;
    if not_after - now > EXPIRY_WINDOW_SECS {
        return None;
    }
    let strategy = match cert.origin {
        TlsCertOrigin::Manual => "manual",
        TlsCertOrigin::Csr => "csr",
        TlsCertOrigin::AcmeDns => unreachable!("filtered above"),
    };
    let description = format!(
        "certificate for {} (strategy: {strategy}) expires at unix={not_after}; \
         the runtime cannot autonomously renew this cert — upload a replacement",
        target.hostname
    );
    Some(DesiredFault {
        app: target.app.clone(),
        ingress_name: target.ingress_name.clone(),
        description,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::secrets::Cipher;
    use crate::runtime::tls::store::{self, CertMetadata};
    use crate::runtime::tls::{KeyType, TlsCertState};

    fn fresh_db() -> (Db, Cipher) {
        let db = Db::open_in_memory().unwrap();
        (db, Cipher::for_tests())
    }

    fn insert_cert(db: &Db, hostname: &str, origin: TlsCertOrigin, not_after: i64) -> i64 {
        store::insert_certificate(
            db,
            hostname,
            TlsCertState::Active,
            origin,
            Some(&self_signed_cert_pem(hostname)),
            None,
            b"key",
            KeyType::EcdsaP256,
            CertMetadata {
                issuer: Some("test".to_owned()),
                not_before: Some(not_after - 90 * 86400),
                not_after: Some(not_after),
                serial: Some("01".to_owned()),
                self_signed: true,
            },
            None,
            None,
        )
        .unwrap()
    }

    fn self_signed_cert_pem(host: &str) -> String {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = rcgen::CertificateParams::new(vec![host.to_owned()]).unwrap();
        params.distinguished_name = rcgen::DistinguishedName::new();
        let cert = params.self_signed(&key).unwrap();
        cert.pem()
    }

    fn target(app: &str, hostname: &str) -> IngressTarget {
        IngressTarget {
            app: AppName::new(app).unwrap(),
            ingress_name: format!("{app}-ingress"),
            hostname: hostname.to_owned(),
        }
    }

    fn active_count(db: &Db, kind: &str) -> usize {
        faults::list_active_faults(db, None)
            .unwrap()
            .into_iter()
            .filter(|f| f.kind == kind)
            .count()
    }

    // r[verify tls.fault.expiring]
    #[test]
    fn files_fault_for_manual_cert_within_window() {
        let (db, _) = fresh_db();
        let now = Timestamp::now().as_second();
        // 7 days remaining → within 14-day window.
        insert_cert(
            &db,
            "foo.example.com",
            TlsCertOrigin::Manual,
            now + 7 * 86400,
        );

        sweep(&db, &[target("alpha", "foo.example.com")]).unwrap();
        assert_eq!(active_count(&db, FAULT_KIND), 1);
    }

    #[test]
    fn no_fault_for_manual_cert_outside_window() {
        let (db, _) = fresh_db();
        let now = Timestamp::now().as_second();
        // 30 days remaining → outside the window.
        insert_cert(
            &db,
            "foo.example.com",
            TlsCertOrigin::Manual,
            now + 30 * 86400,
        );

        sweep(&db, &[target("alpha", "foo.example.com")]).unwrap();
        assert_eq!(active_count(&db, FAULT_KIND), 0);
    }

    #[test]
    fn no_fault_for_acme_dns_origin() {
        let (db, _) = fresh_db();
        let now = Timestamp::now().as_second();
        insert_cert(
            &db,
            "foo.example.com",
            TlsCertOrigin::AcmeDns,
            now + 1 * 86400,
        );

        sweep(&db, &[target("alpha", "foo.example.com")]).unwrap();
        assert_eq!(active_count(&db, FAULT_KIND), 0);
    }

    #[test]
    fn clears_fault_when_cert_no_longer_expiring() {
        let (db, _) = fresh_db();
        let now = Timestamp::now().as_second();
        insert_cert(
            &db,
            "foo.example.com",
            TlsCertOrigin::Manual,
            now + 7 * 86400,
        );
        sweep(&db, &[target("alpha", "foo.example.com")]).unwrap();
        assert_eq!(active_count(&db, FAULT_KIND), 1);

        // Replace with a long-lived cert; the new active row is the one
        // resolution picks up. Old one stays active in DB until it's
        // explicitly superseded — for this test, just bump it to
        // superseded so the SAN-aware lookup finds the fresh one.
        let _new_id = insert_cert(
            &db,
            "foo.example.com",
            TlsCertOrigin::Manual,
            now + 90 * 86400,
        );
        // Mark older cert as superseded so the resolver picks the new one.
        let mut stmt = db
            .conn
            .prepare("UPDATE tls_certificates SET state = 'superseded' WHERE id = ?1")
            .unwrap();
        stmt.execute([1i64]).unwrap();

        sweep(&db, &[target("alpha", "foo.example.com")]).unwrap();
        assert_eq!(active_count(&db, FAULT_KIND), 0);
    }

    #[test]
    fn clears_fault_when_ingress_no_longer_references_hostname() {
        let (db, _) = fresh_db();
        let now = Timestamp::now().as_second();
        insert_cert(
            &db,
            "foo.example.com",
            TlsCertOrigin::Manual,
            now + 7 * 86400,
        );
        sweep(&db, &[target("alpha", "foo.example.com")]).unwrap();
        assert_eq!(active_count(&db, FAULT_KIND), 1);

        // Same target list with no entries: fault should clear.
        sweep(&db, &[]).unwrap();
        assert_eq!(active_count(&db, FAULT_KIND), 0);
    }

    #[test]
    fn idempotent_does_not_duplicate_filed_faults() {
        let (db, _) = fresh_db();
        let now = Timestamp::now().as_second();
        insert_cert(
            &db,
            "foo.example.com",
            TlsCertOrigin::Manual,
            now + 7 * 86400,
        );
        let t = target("alpha", "foo.example.com");

        sweep(&db, std::slice::from_ref(&t)).unwrap();
        sweep(&db, std::slice::from_ref(&t)).unwrap();
        sweep(&db, std::slice::from_ref(&t)).unwrap();
        assert_eq!(active_count(&db, FAULT_KIND), 1);
    }
}
