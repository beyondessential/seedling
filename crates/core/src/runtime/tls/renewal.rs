//! Background renewal task for daemon-issued ACME-DNS certificates.
//!
//! On a fixed cadence (default: hourly), scan `tls_certificates` for active
//! rows whose `origin = 'acme_dns'` and whose remaining validity is less
//! than a configurable fraction of the cert's total lifetime. For each
//! such row, look up the bound DNS provider (via the policy row) and the
//! ACME account that originally issued it, then re-run [`super::acme::issue`].
//!
//! On success the new certificate row supersedes the old one atomically;
//! the next handshake against the hostname picks up the new cert via the
//! `get_certificate` endpoint without reconciler involvement.
//!
//! On failure the old cert remains active and the renewal task retries on
//! the next tick. Persistent failures will eventually trip the
//! [`tls.fault.expiring`] fault once the cert is within 14 days of
//! expiry; that's surfaced via the fault layer in phase 5.

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;

use super::{
    acme::{self, IssueParams},
    store,
};
use crate::runtime::{
    db::{Db, DbHandle},
    secrets::Cipher,
};

/// Default time between renewal scans.
pub const DEFAULT_TICK: Duration = Duration::from_secs(3600);

/// A cert is renewed when its remaining lifetime falls below this fraction
/// of its total lifetime. 1/3 is the common ACME-client convention and copes
/// equally well with 90-day and 6-day cert profiles.
pub const RENEW_AT_FRACTION: f64 = 1.0 / 3.0;

/// Configuration captured at startup so the renewal task can drive issuance
/// without consulting global state.
#[derive(Debug, Clone)]
pub struct RenewalConfig {
    /// ACME directory URL to use for renewals. Reads from existing certs'
    /// `acme_account_id`; this is only used as a fallback / future-proofing.
    pub directory_url: String,
    /// Operator contact email for ACME account registration.
    pub contact_email: String,
}

impl Default for RenewalConfig {
    fn default() -> Self {
        Self {
            directory_url: acme::default_directory_url(),
            contact_email: String::new(),
        }
    }
}

/// Run a single renewal pass: scan for acme-dns certs nearing expiry, and
/// for each, attempt issuance. Returns the number of certs renewed, the
/// number that failed, and any errors encountered (for logging).
// r[impl tls.acme.renewal.auto]
pub async fn tick(db: &DbHandle, cipher: &Cipher, config: &RenewalConfig) -> RenewalReport {
    let mut report = RenewalReport::default();

    let candidates = match db.call(collect_renewal_candidates) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "renewal: failed to enumerate certs");
            return report;
        }
    };

    for cand in candidates {
        match run_one(db, cipher, config, &cand).await {
            Ok(()) => report.renewed += 1,
            Err(e) => {
                tracing::warn!(
                    hostname = %cand.hostname,
                    error = %e,
                    "renewal: issuance failed; will retry on next tick"
                );
                report.failed += 1;
            }
        }
    }

    report
}

/// Spawn the renewal task on the current Tokio runtime. The task runs
/// forever, ticking every `tick_period`.
pub fn spawn(
    db: DbHandle,
    cipher: Arc<Cipher>,
    config: RenewalConfig,
    tick_period: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(tick_period);
        // The first tick fires immediately; skip it so we don't try to renew
        // before the daemon's other systems have warmed up.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let report = tick(&db, &cipher, &config).await;
            if report.renewed > 0 || report.failed > 0 {
                tracing::info!(
                    renewed = report.renewed,
                    failed = report.failed,
                    "tls: renewal pass complete"
                );
            }
        }
    })
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RenewalReport {
    pub renewed: u32,
    pub failed: u32,
}

#[derive(Debug)]
struct Candidate {
    hostname: String,
    contact_email: Option<String>,
    directory_url: Option<String>,
    dns_provider_name: String,
}

fn collect_renewal_candidates(conn: &Db) -> rusqlite::Result<Vec<Candidate>> {
    let now = Timestamp::now().as_second();

    // Pull active acme_dns certs joined with their policy row (for the bound
    // DNS provider) and ACME account (for the directory + email). Only
    // certs whose policy is still acme_dns are renewable here.
    let mut stmt = conn.conn.prepare(
        "SELECT c.hostname, c.not_before, c.not_after,
                a.directory_url, a.contact_email,
                p.dns_provider
         FROM tls_certificates c
         JOIN tls_policies p
           ON p.hostname = c.hostname AND p.strategy = 'acme_dns'
         LEFT JOIN tls_acme_accounts a ON a.id = c.acme_account_id
         WHERE c.state = 'active' AND c.origin = 'acme_dns'",
    )?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut out = Vec::new();
    for (hostname, not_before, not_after, directory_url, contact_email, provider) in rows {
        let Some(provider) = provider else { continue };
        let Some(not_before) = not_before else {
            continue;
        };
        let Some(not_after) = not_after else { continue };
        if not_after <= not_before {
            continue;
        }
        let total = (not_after - not_before) as f64;
        let remaining = (not_after - now) as f64;
        if remaining <= 0.0 || (remaining / total) <= RENEW_AT_FRACTION {
            out.push(Candidate {
                hostname,
                contact_email,
                directory_url,
                dns_provider_name: provider,
            });
        }
    }
    Ok(out)
}

async fn run_one(
    db: &DbHandle,
    cipher: &Cipher,
    config: &RenewalConfig,
    cand: &Candidate,
) -> Result<(), acme::AcmeError> {
    let directory_url = cand
        .directory_url
        .clone()
        .unwrap_or_else(|| config.directory_url.clone());
    let contact_email = cand
        .contact_email
        .clone()
        .unwrap_or_else(|| config.contact_email.clone());

    let issue_params = IssueParams {
        hostname: &cand.hostname,
        contact_email: &contact_email,
        directory_url: &directory_url,
        dns_provider_name: &cand.dns_provider_name,
    };
    acme::issue(db, cipher, issue_params).await.map(|_| ())
}

/// Verify (in tests) that the candidate-selection SQL only picks up rows
/// matching the join + state predicates.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tls::store::CertMetadata;
    use crate::runtime::tls::{KeyType, TlsCertOrigin, TlsCertState};
    use secrecy::SecretString;

    fn fresh_db() -> (Db, Cipher) {
        let db = Db::open_in_memory().unwrap();
        let cipher = Cipher::for_tests();
        (db, cipher)
    }

    fn fake_account(db: &Db, cipher: &Cipher) -> i64 {
        store::insert_acme_account(
            db,
            cipher,
            "https://acme-v02.api.letsencrypt.org/directory",
            "ops@example.com",
            "https://acme-v02.api.letsencrypt.org/acme/acct/123",
            &SecretString::new("dummy".into()),
        )
        .unwrap()
    }

    fn provider_config() -> SecretString {
        SecretString::new(
            r#"{"access_key_id":"AKIA","secret_access_key":"s","region":"us-east-1"}"#.into(),
        )
    }

    fn insert_acme_cert(
        db: &Db,
        hostname: &str,
        not_before: i64,
        not_after: i64,
        account_id: i64,
    ) -> i64 {
        store::insert_certificate(
            db,
            hostname,
            TlsCertState::Active,
            TlsCertOrigin::AcmeDns,
            Some("PEM"),
            None,
            b"key",
            KeyType::EcdsaP256,
            CertMetadata {
                issuer: Some("Let's Encrypt".to_owned()),
                not_before: Some(not_before),
                not_after: Some(not_after),
                serial: Some("01".to_owned()),
                self_signed: false,
            },
            None,
            Some(account_id),
        )
        .unwrap()
    }

    #[test]
    fn candidates_includes_certs_within_renewal_window() {
        let (db, cipher) = fresh_db();
        let aid = fake_account(&db, &cipher);
        store::upsert_dns_provider(
            &db,
            &cipher,
            "p",
            super::super::DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();

        let now = Timestamp::now().as_second();
        // 90-day cert with 25 days left → past 1/3 threshold (30 days), should renew.
        let _ = insert_acme_cert(
            &db,
            "renew.example.com",
            now - 65 * 86400,
            now + 25 * 86400,
            aid,
        );
        store::set_policy_acme_dns(&db, "renew.example.com", "p").unwrap();

        // 90-day cert with 80 days left → well above threshold, skip.
        let _ = insert_acme_cert(
            &db,
            "fresh.example.com",
            now - 10 * 86400,
            now + 80 * 86400,
            aid,
        );
        store::set_policy_acme_dns(&db, "fresh.example.com", "p").unwrap();

        let cands = collect_renewal_candidates(&db).unwrap();
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].hostname, "renew.example.com");
        assert_eq!(cands[0].dns_provider_name, "p");
    }

    #[test]
    fn candidates_excludes_manual_origin() {
        let (db, cipher) = fresh_db();
        let aid = fake_account(&db, &cipher);
        store::upsert_dns_provider(
            &db,
            &cipher,
            "p",
            super::super::DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();

        let now = Timestamp::now().as_second();
        let id = store::insert_certificate(
            &db,
            "manual.example.com",
            TlsCertState::Active,
            TlsCertOrigin::Manual,
            Some("PEM"),
            None,
            b"key",
            KeyType::EcdsaP256,
            CertMetadata {
                not_before: Some(now - 60 * 86400),
                not_after: Some(now + 1 * 86400),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
        store::set_policy_manual(&db, "manual.example.com", id).unwrap();

        // Also have an acme-dns cert outside the window for control.
        let _ = insert_acme_cert(
            &db,
            "fresh.example.com",
            now - 10 * 86400,
            now + 80 * 86400,
            aid,
        );
        store::set_policy_acme_dns(&db, "fresh.example.com", "p").unwrap();

        let cands = collect_renewal_candidates(&db).unwrap();
        assert!(
            cands.is_empty(),
            "manual certs are not renewed; got: {cands:?}"
        );
    }

    #[test]
    fn candidates_excludes_already_superseded() {
        let (db, cipher) = fresh_db();
        let aid = fake_account(&db, &cipher);
        store::upsert_dns_provider(
            &db,
            &cipher,
            "p",
            super::super::DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();

        let now = Timestamp::now().as_second();
        let id = insert_acme_cert(
            &db,
            "old.example.com",
            now - 65 * 86400,
            now + 5 * 86400,
            aid,
        );
        store::update_certificate(&db, id, TlsCertState::Superseded, None, None).unwrap();
        store::set_policy_acme_dns(&db, "old.example.com", "p").unwrap();

        let cands = collect_renewal_candidates(&db).unwrap();
        assert!(cands.is_empty());
    }
}
