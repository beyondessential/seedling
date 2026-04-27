//! Database CRUD for TLS provider, certificate, policy, and ACME-account rows.
//!
//! All credential and key material passes through [`Cipher`] at the
//! storage boundary; in-memory representations carry plaintext only when
//! actively in use.

use jiff::Timestamp;
use rusqlite::{OptionalExtension, params};
use secrecy::{ExposeSecret, SecretString};

use super::{
    AcmeAccount, AttemptOutcome, AttemptTrigger, DnsProviderEntry, DnsProviderKind,
    DnsProviderSummary, KeyType, RetryBlockSource, TlsCertAttempt, TlsCertForceRetry,
    TlsCertOrigin, TlsCertRetryBlock, TlsCertState, TlsCertificate, TlsPolicy, TlsPolicyRow,
    TlsSettings, pattern_matches, pattern_specificity,
};
use crate::runtime::{db::Db, secrets::Cipher};

fn now_secs() -> i64 {
    Timestamp::now().as_second()
}

// ---------------------------------------------------------------------------
// DNS providers
// ---------------------------------------------------------------------------

// r[impl tls.dns-provider.lifecycle]
pub fn list_dns_providers(db: &Db) -> rusqlite::Result<Vec<DnsProviderSummary>> {
    let mut stmt = db.conn.prepare(
        "SELECT name, kind, created_at, updated_at FROM tls_dns_providers ORDER BY name",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let kind_str: String = row.get(1)?;
            Ok(DnsProviderSummary {
                name: row.get(0)?,
                kind: DnsProviderKind::parse(&kind_str)
                    .ok_or_else(|| rusqlite::Error::InvalidQuery)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Like [`get_dns_provider`] but returns the encrypted blob without
/// decrypting. Suitable for DB-thread closures that don't hold the cipher.
pub fn get_dns_provider_raw(db: &Db, name: &str) -> rusqlite::Result<Option<DnsProviderRaw>> {
    db.conn
        .query_row(
            "SELECT name, kind, config_ciphertext, created_at, updated_at
             FROM tls_dns_providers WHERE name = ?1",
            [name],
            |row| {
                let kind_str: String = row.get(1)?;
                Ok(DnsProviderRaw {
                    name: row.get(0)?,
                    kind: DnsProviderKind::parse(&kind_str)
                        .ok_or_else(|| rusqlite::Error::InvalidQuery)?,
                    config_ciphertext: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .optional()
}

#[derive(Debug, Clone)]
pub struct DnsProviderRaw {
    pub name: String,
    pub kind: DnsProviderKind,
    pub config_ciphertext: Vec<u8>,
    pub created_at: i64,
    pub updated_at: i64,
}

// r[impl tls.dns-provider.lifecycle]
pub fn get_dns_provider(
    db: &Db,
    cipher: &Cipher,
    name: &str,
) -> rusqlite::Result<Option<DnsProviderEntry>> {
    let row = db
        .conn
        .query_row(
            "SELECT name, kind, config_ciphertext, created_at, updated_at
             FROM tls_dns_providers WHERE name = ?1",
            [name],
            |row| {
                let kind_str: String = row.get(1)?;
                let ct: Vec<u8> = row.get(2)?;
                let kind = DnsProviderKind::parse(&kind_str)
                    .ok_or_else(|| rusqlite::Error::InvalidQuery)?;
                let config = cipher
                    .decrypt(&ct)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(DnsProviderEntry {
                    name: row.get(0)?,
                    kind,
                    config,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Outcome of [`upsert_dns_provider`]. Lets the caller surface to operators
/// when a default `*` policy was auto-created so the first ACME-DNS setup
/// is a single step rather than two.
#[derive(Debug, Clone, Copy, Default)]
pub struct UpsertProviderOutcome {
    pub auto_policy_created: bool,
}

// r[impl tls.dns-provider.lifecycle]
// r[impl tls.policy.auto-default]
pub fn upsert_dns_provider(
    db: &Db,
    cipher: &Cipher,
    name: &str,
    kind: DnsProviderKind,
    config: &SecretString,
) -> rusqlite::Result<UpsertProviderOutcome> {
    let ct = cipher
        .encrypt(config)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let now = now_secs();

    // Wrap insert + auto-policy in a transaction so a partial state
    // (provider inserted, policy missed) cannot persist.
    let tx = db.conn.unchecked_transaction()?;

    // Snapshot whether any providers existed before this upsert. If not,
    // and no policy currently covers `*`, we'll add a catch-all policy
    // pointing at this provider so all hostnames flow through ACME-DNS by
    // default. Operators can clear or replace it any time.
    let providers_before: i64 =
        tx.query_row("SELECT COUNT(*) FROM tls_dns_providers", [], |r| r.get(0))?;
    let star_policy_exists: i64 = tx.query_row(
        "SELECT COUNT(*) FROM tls_policies WHERE hostname = '*'",
        [],
        |r| r.get(0),
    )?;

    tx.execute(
        "INSERT INTO tls_dns_providers (name, kind, config_ciphertext, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(name) DO UPDATE SET
             kind = excluded.kind,
             config_ciphertext = excluded.config_ciphertext,
             updated_at = excluded.updated_at",
        params![name, kind.as_str(), ct, now],
    )?;

    let mut auto_policy_created = false;
    if providers_before == 0 && star_policy_exists == 0 {
        // r[impl tls.policy.auto-default]
        tx.execute(
            "INSERT INTO tls_policies (hostname, strategy, dns_provider, cert_id, updated_at)
             VALUES ('*', 'acme_dns', ?1, NULL, ?2)",
            params![name, now],
        )?;
        auto_policy_created = true;
    }
    tx.commit()?;
    Ok(UpsertProviderOutcome {
        auto_policy_created,
    })
}

// r[impl tls.dns-provider.lifecycle]
/// Refused by FK if any policy references this provider.
pub fn delete_dns_provider(db: &Db, name: &str) -> rusqlite::Result<bool> {
    let n = db
        .conn
        .execute("DELETE FROM tls_dns_providers WHERE name = ?1", [name])?;
    Ok(n > 0)
}

// ---------------------------------------------------------------------------
// Policies
// ---------------------------------------------------------------------------

// r[impl tls.strategy.acme-dns]
// r[impl tls.strategy.manual]
pub fn list_policies(db: &Db) -> rusqlite::Result<Vec<TlsPolicyRow>> {
    let mut stmt = db.conn.prepare(
        "SELECT hostname, strategy, dns_provider, cert_id, updated_at
         FROM tls_policies ORDER BY hostname",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let hostname: String = row.get(0)?;
            let strategy: String = row.get(1)?;
            let dns_provider: Option<String> = row.get(2)?;
            let _cert_id: Option<i64> = row.get(3)?;
            let updated_at: i64 = row.get(4)?;
            // Manual policy rows from older shipped versions of the
            // schema (where strategy = 'manual' + cert_id) are now
            // ignored: manual certs auto-bind by SAN coverage at
            // resolution time. Returning Ok(None) drops them; the
            // outer collect filters None out.
            let policy = match strategy.as_str() {
                "acme_dns" => TlsPolicy::AcmeDns {
                    dns_provider: dns_provider.ok_or_else(|| rusqlite::Error::InvalidQuery)?,
                },
                "manual" => return Ok(None),
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            Ok(Some(TlsPolicyRow {
                hostname,
                policy,
                updated_at,
            }))
        })?
        .filter_map(|r| r.transpose())
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub fn get_policy(db: &Db, hostname: &str) -> rusqlite::Result<Option<TlsPolicyRow>> {
    let policies = list_policies(db)?;
    Ok(policies.into_iter().find(|p| p.hostname == hostname))
}

// r[impl tls.strategy.acme-dns]
// r[impl tls.policy.apply]
pub fn set_policy_acme_dns(db: &Db, hostname: &str, dns_provider: &str) -> rusqlite::Result<()> {
    let now = now_secs();
    db.conn.execute(
        "INSERT INTO tls_policies (hostname, strategy, dns_provider, cert_id, updated_at)
         VALUES (?1, 'acme_dns', ?2, NULL, ?3)
         ON CONFLICT(hostname) DO UPDATE SET
             strategy = excluded.strategy,
             dns_provider = excluded.dns_provider,
             cert_id = NULL,
             updated_at = excluded.updated_at",
        params![hostname, dns_provider, now],
    )?;
    Ok(())
}

// r[impl tls.policy.apply]
pub fn clear_policy(db: &Db, hostname: &str) -> rusqlite::Result<bool> {
    let n = db
        .conn
        .execute("DELETE FROM tls_policies WHERE hostname = ?1", [hostname])?;
    Ok(n > 0)
}

// ---------------------------------------------------------------------------
// Certificates
// ---------------------------------------------------------------------------

#[expect(clippy::too_many_arguments, reason = "cert rows have many fields")]
pub fn insert_certificate(
    db: &Db,
    hostname: &str,
    state: TlsCertState,
    origin: TlsCertOrigin,
    cert_pem: Option<&str>,
    csr_pem: Option<&str>,
    key_ciphertext: &[u8],
    key_type: KeyType,
    metadata: CertMetadata,
    note: Option<&str>,
    acme_account_id: Option<i64>,
) -> rusqlite::Result<i64> {
    let now = now_secs();
    db.conn.execute(
        "INSERT INTO tls_certificates (
            hostname, state, origin, cert_pem, csr_pem, key_ciphertext,
            key_type, issuer, not_before, not_after, serial, self_signed,
            note, acme_account_id, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
        params![
            hostname,
            state.as_str(),
            origin.as_str(),
            cert_pem,
            csr_pem,
            key_ciphertext,
            key_type.as_str(),
            metadata.issuer,
            metadata.not_before,
            metadata.not_after,
            metadata.serial,
            metadata.self_signed as i64,
            note,
            acme_account_id,
            now,
        ],
    )?;
    Ok(db.conn.last_insert_rowid())
}

#[derive(Debug, Clone, Default)]
pub struct CertMetadata {
    pub issuer: Option<String>,
    pub not_before: Option<i64>,
    pub not_after: Option<i64>,
    pub serial: Option<String>,
    pub self_signed: bool,
}

pub fn get_certificate(db: &Db, id: i64) -> rusqlite::Result<Option<TlsCertificate>> {
    db.conn
        .query_row(
            "SELECT id, hostname, state, origin, cert_pem, csr_pem, key_ciphertext,
                    key_type, issuer, not_before, not_after, serial, self_signed,
                    note, acme_account_id, ari_window_start, ari_window_end,
                    ari_polled_at, created_at, updated_at
             FROM tls_certificates WHERE id = ?1",
            [id],
            row_to_certificate,
        )
        .optional()
}

pub fn list_certificates(db: &Db) -> rusqlite::Result<Vec<TlsCertificate>> {
    let mut stmt = db.conn.prepare(
        "SELECT id, hostname, state, origin, cert_pem, csr_pem, key_ciphertext,
                key_type, issuer, not_before, not_after, serial, self_signed,
                note, acme_account_id, ari_window_start, ari_window_end,
                ari_polled_at, created_at, updated_at
         FROM tls_certificates ORDER BY id DESC",
    )?;
    stmt.query_map([], row_to_certificate)?.collect()
}

/// Returns the most-recent active cert covering `hostname`, if any.
///
/// Resolution rules:
///
/// - Exact match on the cert's primary `hostname` column wins (this is
///   the fast path for ACME-DNS certs, whose row is always created
///   for the hostname they were issued for).
/// - Otherwise, scan every active cert and pick the most-recent one
///   whose SubjectAlternativeName list covers `hostname` per RFC 6125
///   (literal match or single-label wildcard). This auto-binds manual
///   uploads — including wildcard certs — without requiring the
///   operator to re-declare the binding per host.
// r[impl tls.strategy.manual]
pub fn find_active_for_hostname(
    db: &Db,
    hostname: &str,
) -> rusqlite::Result<Option<TlsCertificate>> {
    if let Some(cert) = db
        .conn
        .query_row(
            "SELECT id, hostname, state, origin, cert_pem, csr_pem, key_ciphertext,
                    key_type, issuer, not_before, not_after, serial, self_signed,
                    note, acme_account_id, ari_window_start, ari_window_end,
                    ari_polled_at, created_at, updated_at
             FROM tls_certificates
             WHERE hostname = ?1 AND state = 'active'
             ORDER BY id DESC LIMIT 1",
            [hostname],
            row_to_certificate,
        )
        .optional()?
    {
        return Ok(Some(cert));
    }

    // SAN-coverage scan: walk active certs newest-first and return the
    // first whose SAN list covers the hostname. Cost is one PEM parse
    // per active row; in operator-scale databases (<<1000 active rows)
    // this is microseconds.
    let mut stmt = db.conn.prepare(
        "SELECT id, hostname, state, origin, cert_pem, csr_pem, key_ciphertext,
                key_type, issuer, not_before, not_after, serial, self_signed,
                note, acme_account_id, ari_window_start, ari_window_end,
                ari_polled_at, created_at, updated_at
         FROM tls_certificates
         WHERE state = 'active'
         ORDER BY created_at DESC, id DESC",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let cert = row_to_certificate(row)?;
        let Some(pem) = cert.cert_pem.as_deref() else {
            continue;
        };
        let Ok(parsed) = super::parse::parse_chain(pem) else {
            continue;
        };
        if super::parse::san_covers(&parsed.san_dns_names, hostname) {
            return Ok(Some(cert));
        }
    }
    Ok(None)
}

/// Transition a cert to a new state, optionally updating cert PEM and parsed
/// metadata. Used by:
///
/// - CSR upload: pending → active, supplying cert_pem + parsed metadata.
/// - ACME renewal: active → superseded for the old row.
/// - Validation failure on CSR upload: pending → failed.
pub fn update_certificate(
    db: &Db,
    id: i64,
    state: TlsCertState,
    cert_pem: Option<&str>,
    metadata: Option<&CertMetadata>,
) -> rusqlite::Result<()> {
    let now = now_secs();
    db.conn.execute(
        "UPDATE tls_certificates SET
            state = ?1,
            cert_pem = COALESCE(?2, cert_pem),
            issuer = COALESCE(?3, issuer),
            not_before = COALESCE(?4, not_before),
            not_after = COALESCE(?5, not_after),
            serial = COALESCE(?6, serial),
            self_signed = COALESCE(?7, self_signed),
            updated_at = ?8
         WHERE id = ?9",
        params![
            state.as_str(),
            cert_pem,
            metadata.and_then(|m| m.issuer.as_deref()),
            metadata.and_then(|m| m.not_before),
            metadata.and_then(|m| m.not_after),
            metadata.and_then(|m| m.serial.as_deref()),
            metadata.map(|m| m.self_signed as i64),
            now,
            id,
        ],
    )?;
    Ok(())
}

/// Mark all currently-active certs for `hostname` (other than `keep_id`) as
/// superseded. Called after a successful renewal/upload so handshakes pick up
/// the new cert and the old one moves to history.
pub fn supersede_other_active_for_hostname(
    db: &Db,
    hostname: &str,
    keep_id: i64,
) -> rusqlite::Result<usize> {
    let now = now_secs();
    let n = db.conn.execute(
        "UPDATE tls_certificates SET state = 'superseded', updated_at = ?1
         WHERE hostname = ?2 AND state = 'active' AND id != ?3",
        params![now, hostname, keep_id],
    )?;
    Ok(n)
}

pub fn delete_certificate(db: &Db, id: i64) -> rusqlite::Result<bool> {
    let n = db
        .conn
        .execute("DELETE FROM tls_certificates WHERE id = ?1", [id])?;
    Ok(n > 0)
}

fn row_to_certificate(row: &rusqlite::Row<'_>) -> rusqlite::Result<TlsCertificate> {
    let state_str: String = row.get(2)?;
    let origin_str: String = row.get(3)?;
    let key_type_str: String = row.get(7)?;
    let self_signed_int: i64 = row.get(12)?;
    Ok(TlsCertificate {
        id: row.get(0)?,
        hostname: row.get(1)?,
        state: TlsCertState::parse(&state_str).ok_or(rusqlite::Error::InvalidQuery)?,
        origin: TlsCertOrigin::parse(&origin_str).ok_or(rusqlite::Error::InvalidQuery)?,
        cert_pem: row.get(4)?,
        csr_pem: row.get(5)?,
        key_ciphertext: row.get(6)?,
        key_type: KeyType::parse(&key_type_str).ok_or(rusqlite::Error::InvalidQuery)?,
        issuer: row.get(8)?,
        not_before: row.get(9)?,
        not_after: row.get(10)?,
        serial: row.get(11)?,
        self_signed: self_signed_int != 0,
        note: row.get(13)?,
        acme_account_id: row.get(14)?,
        ari_window_start: row.get(15)?,
        ari_window_end: row.get(16)?,
        ari_polled_at: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
    })
}

/// Update the ARI suggested-renewal window for a cert. `polled_at` is the
/// time the data was fetched from the CA, which the renewal task uses to
/// decide when to re-poll.
// r[impl tls.cert.ari]
pub fn update_ari_window(
    db: &Db,
    id: i64,
    window_start: i64,
    window_end: i64,
    polled_at: i64,
) -> rusqlite::Result<()> {
    db.conn.execute(
        "UPDATE tls_certificates SET
            ari_window_start = ?1,
            ari_window_end   = ?2,
            ari_polled_at    = ?3,
            updated_at       = ?3
         WHERE id = ?4",
        params![window_start, window_end, polled_at, id],
    )?;
    Ok(())
}

/// Find the most-specific [`TlsPolicy`] that matches `hostname`. Patterns
/// are evaluated under the rules defined in [`super::pattern_matches`]:
/// exact > `*.suffix` (longest suffix wins) > `*`. Returns `None` when no
/// policy matches, signalling the runtime default ACME-HTTP-01.
// r[impl tls.policy.wildcard]
pub fn resolve_policy(db: &Db, hostname: &str) -> rusqlite::Result<Option<TlsPolicyRow>> {
    let policies = list_policies(db)?;
    let mut best: Option<(u32, TlsPolicyRow)> = None;
    for row in policies {
        if pattern_matches(&row.hostname, hostname) {
            let score = pattern_specificity(&row.hostname);
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                best = Some((score, row));
            }
        }
    }
    Ok(best.map(|(_, row)| row))
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

// r[impl tls.settings.contact-email]
// r[impl tls.settings.cert-profile]
pub fn get_settings(db: &Db) -> rusqlite::Result<TlsSettings> {
    db.conn.query_row(
        "SELECT contact_email, cert_profile, updated_at FROM tls_settings WHERE singleton = 1",
        [],
        |row| {
            // Stored empty string normalises to None so callers don't
            // have to differentiate "explicitly cleared" from "never set".
            let raw_profile: Option<String> = row.get(1)?;
            let cert_profile = raw_profile.filter(|s| !s.is_empty());
            Ok(TlsSettings {
                contact_email: row.get(0)?,
                cert_profile,
                updated_at: row.get(2)?,
            })
        },
    )
}

// r[impl tls.settings.contact-email]
pub fn set_contact_email(db: &Db, email: &str) -> rusqlite::Result<()> {
    let now = now_secs();
    db.conn.execute(
        "UPDATE tls_settings SET contact_email = ?1, updated_at = ?2 WHERE singleton = 1",
        params![email, now],
    )?;
    Ok(())
}

// r[impl tls.settings.cert-profile]
pub fn set_cert_profile(db: &Db, profile: Option<&str>) -> rusqlite::Result<()> {
    let now = now_secs();
    let stored = profile.map(str::trim).filter(|s| !s.is_empty());
    db.conn.execute(
        "UPDATE tls_settings SET cert_profile = ?1, updated_at = ?2 WHERE singleton = 1",
        params![stored, now],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// ACME accounts
// ---------------------------------------------------------------------------

// r[impl tls.acme.account.persist]
pub fn get_acme_account(
    db: &Db,
    directory_url: &str,
    contact_email: &str,
) -> rusqlite::Result<Option<AcmeAccount>> {
    db.conn
        .query_row(
            "SELECT id, directory_url, contact_email, account_url,
                    account_key_ciphertext, created_at, updated_at
             FROM tls_acme_accounts
             WHERE directory_url = ?1 AND contact_email = ?2",
            params![directory_url, contact_email],
            row_to_acme_account,
        )
        .optional()
}

pub fn get_acme_account_by_id(db: &Db, id: i64) -> rusqlite::Result<Option<AcmeAccount>> {
    db.conn
        .query_row(
            "SELECT id, directory_url, contact_email, account_url,
                    account_key_ciphertext, created_at, updated_at
             FROM tls_acme_accounts WHERE id = ?1",
            [id],
            row_to_acme_account,
        )
        .optional()
}

// r[impl tls.acme.account.persist]
pub fn insert_acme_account(
    db: &Db,
    cipher: &Cipher,
    directory_url: &str,
    contact_email: &str,
    account_url: &str,
    account_key_pem: &SecretString,
) -> rusqlite::Result<i64> {
    let ct = cipher
        .encrypt(account_key_pem)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    insert_acme_account_raw(db, directory_url, contact_email, account_url, &ct)
}

/// Variant of [`insert_acme_account`] that takes pre-encrypted ciphertext.
/// Use this in DB-thread closures that don't carry a [`Cipher`].
pub fn insert_acme_account_raw(
    db: &Db,
    directory_url: &str,
    contact_email: &str,
    account_url: &str,
    account_key_ciphertext: &[u8],
) -> rusqlite::Result<i64> {
    let now = now_secs();
    db.conn.execute(
        "INSERT INTO tls_acme_accounts
            (directory_url, contact_email, account_key_ciphertext,
             account_url, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![
            directory_url,
            contact_email,
            account_key_ciphertext,
            account_url,
            now
        ],
    )?;
    Ok(db.conn.last_insert_rowid())
}

pub fn decrypt_acme_account_key(
    cipher: &Cipher,
    account: &AcmeAccount,
) -> Result<SecretString, crate::runtime::secrets::Error> {
    cipher.decrypt(&account.account_key_ciphertext)
}

fn row_to_acme_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<AcmeAccount> {
    Ok(AcmeAccount {
        id: row.get(0)?,
        directory_url: row.get(1)?,
        contact_email: row.get(2)?,
        account_url: row.get(3)?,
        account_key_ciphertext: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

// ---------------------------------------------------------------------------
// Cert attempts log
// ---------------------------------------------------------------------------

// r[impl tls.cert.attempt-log]
pub fn insert_attempt(
    db: &Db,
    hostname: &str,
    triggered_by: AttemptTrigger,
) -> rusqlite::Result<i64> {
    let now = now_secs();
    db.conn.execute(
        "INSERT INTO tls_cert_attempts (hostname, triggered_by, started_at, outcome)
         VALUES (?1, ?2, ?3, 'pending')",
        params![hostname, triggered_by.as_str(), now],
    )?;
    Ok(db.conn.last_insert_rowid())
}

// r[impl tls.cert.attempt-log]
pub fn finalize_attempt(
    db: &Db,
    id: i64,
    outcome: AttemptOutcome,
    cert_id: Option<i64>,
    error: Option<&str>,
) -> rusqlite::Result<()> {
    let now = now_secs();
    db.conn.execute(
        "UPDATE tls_cert_attempts SET
            outcome     = ?1,
            cert_id     = ?2,
            error       = ?3,
            finished_at = ?4
         WHERE id = ?5",
        params![outcome.as_str(), cert_id, error, now, id],
    )?;
    Ok(())
}

/// List recent attempts. When `hostname` is `Some`, scopes to that
/// hostname; otherwise returns all attempts. Newest first.
pub fn list_attempts(
    db: &Db,
    hostname: Option<&str>,
    limit: i64,
) -> rusqlite::Result<Vec<TlsCertAttempt>> {
    let (sql, params) = if let Some(host) = hostname {
        (
            "SELECT id, hostname, triggered_by, started_at, finished_at, outcome, cert_id, error
             FROM tls_cert_attempts
             WHERE hostname = ?1
             ORDER BY id DESC LIMIT ?2"
                .to_owned(),
            rusqlite::params_from_iter::<Vec<Box<dyn rusqlite::ToSql>>>(vec![
                Box::new(host.to_owned()),
                Box::new(limit),
            ]),
        )
    } else {
        (
            "SELECT id, hostname, triggered_by, started_at, finished_at, outcome, cert_id, error
             FROM tls_cert_attempts
             ORDER BY id DESC LIMIT ?1"
                .to_owned(),
            rusqlite::params_from_iter::<Vec<Box<dyn rusqlite::ToSql>>>(vec![Box::new(limit)]),
        )
    };
    let mut stmt = db.conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params, |row| {
            let trig: String = row.get(2)?;
            let outc: String = row.get(5)?;
            Ok(TlsCertAttempt {
                id: row.get(0)?,
                hostname: row.get(1)?,
                triggered_by: AttemptTrigger::parse(&trig).ok_or(rusqlite::Error::InvalidQuery)?,
                started_at: row.get(3)?,
                finished_at: row.get(4)?,
                outcome: AttemptOutcome::parse(&outc).ok_or(rusqlite::Error::InvalidQuery)?,
                cert_id: row.get(6)?,
                error: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Retry blocks
// ---------------------------------------------------------------------------

// r[impl tls.cert.retry-block]
pub fn set_retry_block(
    db: &Db,
    hostname: &str,
    set_by: RetryBlockSource,
    reason: Option<&str>,
) -> rusqlite::Result<()> {
    let now = now_secs();
    db.conn.execute(
        "INSERT INTO tls_cert_retry_blocks (hostname, set_at, set_by, reason)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(hostname) DO UPDATE SET
             set_at = excluded.set_at,
             set_by = excluded.set_by,
             reason = excluded.reason",
        params![hostname, now, set_by.as_str(), reason],
    )?;
    Ok(())
}

// r[impl tls.cert.retry-block]
pub fn clear_retry_block(db: &Db, hostname: &str) -> rusqlite::Result<bool> {
    let n = db.conn.execute(
        "DELETE FROM tls_cert_retry_blocks WHERE hostname = ?1",
        [hostname],
    )?;
    Ok(n > 0)
}

pub fn is_retry_blocked(db: &Db, hostname: &str) -> rusqlite::Result<bool> {
    let n: i64 = db.conn.query_row(
        "SELECT COUNT(*) FROM tls_cert_retry_blocks WHERE hostname = ?1",
        [hostname],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

pub fn list_retry_blocks(db: &Db) -> rusqlite::Result<Vec<TlsCertRetryBlock>> {
    let mut stmt = db.conn.prepare(
        "SELECT hostname, set_at, set_by, reason
         FROM tls_cert_retry_blocks ORDER BY hostname",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let set_by_str: String = row.get(2)?;
            Ok(TlsCertRetryBlock {
                hostname: row.get(0)?,
                set_at: row.get(1)?,
                set_by: RetryBlockSource::parse(&set_by_str)
                    .ok_or(rusqlite::Error::InvalidQuery)?,
                reason: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Force-retry signal
// ---------------------------------------------------------------------------

// r[impl tls.cert.force-retry]
pub fn set_force_retry(db: &Db, hostname: &str) -> rusqlite::Result<()> {
    let now = now_secs();
    db.conn.execute(
        "INSERT INTO tls_cert_force_retry (hostname, requested_at)
         VALUES (?1, ?2)
         ON CONFLICT(hostname) DO UPDATE SET requested_at = excluded.requested_at",
        params![hostname, now],
    )?;
    Ok(())
}

/// Return whether `hostname` has a force-retry row, atomically deleting it.
/// The reconciler calls this at the start of an issuance run so a single
/// retry request is consumed exactly once even if multiple ticks race.
// r[impl tls.cert.force-retry]
pub fn take_force_retry(db: &Db, hostname: &str) -> rusqlite::Result<bool> {
    let n = db.conn.execute(
        "DELETE FROM tls_cert_force_retry WHERE hostname = ?1",
        [hostname],
    )?;
    Ok(n > 0)
}

pub fn list_force_retries(db: &Db) -> rusqlite::Result<Vec<TlsCertForceRetry>> {
    let mut stmt = db
        .conn
        .prepare("SELECT hostname, requested_at FROM tls_cert_force_retry ORDER BY hostname")?;
    let rows = stmt
        .query_map([], |row| {
            Ok(TlsCertForceRetry {
                hostname: row.get(0)?,
                requested_at: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::SecretString;

    fn fresh_db() -> (Db, Cipher) {
        let db = Db::open_in_memory().unwrap();
        let cipher = Cipher::for_tests();
        (db, cipher)
    }

    fn provider_config() -> SecretString {
        SecretString::new(
            r#"{"access_key_id":"AKIA","secret_access_key":"secret","region":"us-east-1"}"#.into(),
        )
    }

    #[test]
    fn dns_provider_upsert_then_get_round_trips() {
        let (db, cipher) = fresh_db();
        upsert_dns_provider(
            &db,
            &cipher,
            "primary",
            DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();

        let entry = get_dns_provider(&db, &cipher, "primary").unwrap().unwrap();
        assert_eq!(entry.name, "primary");
        assert_eq!(entry.kind, DnsProviderKind::Route53);
        assert!(entry.config.expose_secret().contains("AKIA"));
    }

    #[test]
    fn dns_provider_list_excludes_credentials() {
        let (db, cipher) = fresh_db();
        upsert_dns_provider(
            &db,
            &cipher,
            "p1",
            DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();
        upsert_dns_provider(
            &db,
            &cipher,
            "p2",
            DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();

        let summaries = list_dns_providers(&db).unwrap();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].name, "p1");
        assert_eq!(summaries[1].name, "p2");
    }

    #[test]
    fn dns_provider_upsert_replaces_on_conflict() {
        let (db, cipher) = fresh_db();
        upsert_dns_provider(
            &db,
            &cipher,
            "p",
            DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();
        let v1 = get_dns_provider(&db, &cipher, "p").unwrap().unwrap();

        let new = SecretString::new(
            r#"{"access_key_id":"AKIA2","secret_access_key":"s","region":"r"}"#.into(),
        );
        upsert_dns_provider(&db, &cipher, "p", DnsProviderKind::Route53, &new).unwrap();
        let v2 = get_dns_provider(&db, &cipher, "p").unwrap().unwrap();

        assert!(v2.config.expose_secret().contains("AKIA2"));
        assert!(v2.updated_at >= v1.updated_at);
    }

    #[test]
    fn dns_provider_delete_returns_true_when_present() {
        let (db, cipher) = fresh_db();
        upsert_dns_provider(
            &db,
            &cipher,
            "p",
            DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();
        // Upsert auto-creates a `*` policy referencing this provider; clear
        // it so the deletion isn't refused by the FK.
        clear_policy(&db, "*").unwrap();
        assert!(delete_dns_provider(&db, "p").unwrap());
        assert!(!delete_dns_provider(&db, "p").unwrap());
    }

    #[test]
    fn dns_provider_delete_refused_while_referenced() {
        let (db, cipher) = fresh_db();
        upsert_dns_provider(
            &db,
            &cipher,
            "p",
            DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();
        set_policy_acme_dns(&db, "foo.example.com", "p").unwrap();

        let err = db
            .conn
            .execute("DELETE FROM tls_dns_providers WHERE name = ?1", ["p"]);
        assert!(
            err.is_err(),
            "FK should refuse delete while a policy points at it"
        );
    }

    fn insert_test_cert(db: &Db, hostname: &str) -> i64 {
        insert_certificate(
            db,
            hostname,
            TlsCertState::Active,
            TlsCertOrigin::Manual,
            Some("-----BEGIN CERTIFICATE-----\nMIIBdummy\n-----END CERTIFICATE-----\n"),
            None,
            b"encrypted-key-bytes",
            KeyType::EcdsaP256,
            CertMetadata {
                issuer: Some("CN=Test CA".to_string()),
                not_before: Some(1_700_000_000),
                not_after: Some(1_800_000_000),
                serial: Some("01".to_string()),
                self_signed: false,
            },
            None,
            None,
        )
        .unwrap()
    }

    #[test]
    fn certificate_insert_and_get_round_trip() {
        let (db, _) = fresh_db();
        let id = insert_test_cert(&db, "foo.example.com");
        let row = get_certificate(&db, id).unwrap().unwrap();
        assert_eq!(row.hostname, "foo.example.com");
        assert_eq!(row.state, TlsCertState::Active);
        assert_eq!(row.origin, TlsCertOrigin::Manual);
        assert_eq!(row.key_type, KeyType::EcdsaP256);
        assert_eq!(row.issuer.as_deref(), Some("CN=Test CA"));
        assert!(!row.self_signed);
    }

    #[test]
    fn find_active_for_hostname_returns_latest() {
        let (db, _) = fresh_db();
        let _id1 = insert_test_cert(&db, "a.example.com");
        let id2 = insert_test_cert(&db, "a.example.com");
        let _id3 = insert_test_cert(&db, "b.example.com");

        let found = find_active_for_hostname(&db, "a.example.com")
            .unwrap()
            .unwrap();
        assert_eq!(found.id, id2);
    }

    #[test]
    fn supersede_other_active_only_touches_target_hostname() {
        let (db, _) = fresh_db();
        let id1 = insert_test_cert(&db, "a.example.com");
        let id2 = insert_test_cert(&db, "a.example.com");
        let id3 = insert_test_cert(&db, "b.example.com");

        let n = supersede_other_active_for_hostname(&db, "a.example.com", id2).unwrap();
        assert_eq!(n, 1);

        assert_eq!(
            get_certificate(&db, id1).unwrap().unwrap().state,
            TlsCertState::Superseded
        );
        assert_eq!(
            get_certificate(&db, id2).unwrap().unwrap().state,
            TlsCertState::Active
        );
        assert_eq!(
            get_certificate(&db, id3).unwrap().unwrap().state,
            TlsCertState::Active
        );
    }

    #[test]
    fn update_certificate_transitions_state_and_metadata() {
        let (db, _) = fresh_db();
        let id = insert_certificate(
            &db,
            "foo.example.com",
            TlsCertState::CsrPending,
            TlsCertOrigin::Csr,
            None,
            Some(
                "-----BEGIN CERTIFICATE REQUEST-----\nMIICSR\n-----END CERTIFICATE REQUEST-----\n",
            ),
            b"key",
            KeyType::EcdsaP256,
            CertMetadata::default(),
            None,
            None,
        )
        .unwrap();

        update_certificate(
            &db,
            id,
            TlsCertState::Active,
            Some("-----BEGIN CERTIFICATE-----\ndata\n-----END CERTIFICATE-----\n"),
            Some(&CertMetadata {
                issuer: Some("CN=Issuer".to_string()),
                not_after: Some(1_900_000_000),
                ..Default::default()
            }),
        )
        .unwrap();

        let row = get_certificate(&db, id).unwrap().unwrap();
        assert_eq!(row.state, TlsCertState::Active);
        assert!(row.cert_pem.unwrap().contains("BEGIN CERTIFICATE"));
        assert_eq!(row.issuer.as_deref(), Some("CN=Issuer"));
        assert_eq!(row.not_after, Some(1_900_000_000));
    }

    #[test]
    fn policy_acme_dns_set_then_list() {
        let (db, cipher) = fresh_db();
        upsert_dns_provider(
            &db,
            &cipher,
            "p",
            DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();
        // Drop the auto-created `*` policy so this test only counts the
        // explicit ones added below.
        clear_policy(&db, "*").unwrap();
        set_policy_acme_dns(&db, "foo.example.com", "p").unwrap();
        set_policy_acme_dns(&db, "bar.example.com", "p").unwrap();

        let rows = list_policies(&db).unwrap();
        assert_eq!(rows.len(), 2);
        match &rows[0].policy {
            TlsPolicy::AcmeDns { dns_provider } => assert_eq!(dns_provider, "p"),
        }
    }

    #[test]
    fn acme_dns_policy_then_clear() {
        let (db, cipher) = fresh_db();
        upsert_dns_provider(
            &db,
            &cipher,
            "p",
            DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();
        clear_policy(&db, "*").unwrap();
        set_policy_acme_dns(&db, "foo.example.com", "p").unwrap();

        let row = get_policy(&db, "foo.example.com").unwrap().unwrap();
        match row.policy {
            TlsPolicy::AcmeDns { dns_provider } => assert_eq!(dns_provider, "p"),
        }

        assert!(clear_policy(&db, "foo.example.com").unwrap());
        assert!(get_policy(&db, "foo.example.com").unwrap().is_none());
    }

    #[test]
    fn acme_account_insert_and_lookup() {
        let (db, cipher) = fresh_db();
        let key_pem = SecretString::new(
            "-----BEGIN PRIVATE KEY-----\ndummy\n-----END PRIVATE KEY-----\n".into(),
        );
        let id = insert_acme_account(
            &db,
            &cipher,
            "https://acme-v02.api.letsencrypt.org/directory",
            "ops@example.com",
            "https://acme-v02.api.letsencrypt.org/acme/acct/12345",
            &key_pem,
        )
        .unwrap();

        let by_pair = get_acme_account(
            &db,
            "https://acme-v02.api.letsencrypt.org/directory",
            "ops@example.com",
        )
        .unwrap()
        .unwrap();
        assert_eq!(by_pair.id, id);

        let by_id = get_acme_account_by_id(&db, id).unwrap().unwrap();
        assert_eq!(by_id.account_url, by_pair.account_url);

        let decrypted = decrypt_acme_account_key(&cipher, &by_id).unwrap();
        assert!(decrypted.expose_secret().contains("BEGIN PRIVATE KEY"));
    }

    // r[verify tls.policy.auto-default]
    #[test]
    fn first_provider_upsert_auto_creates_star_policy() {
        let (db, cipher) = fresh_db();
        let outcome = upsert_dns_provider(
            &db,
            &cipher,
            "primary",
            DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();
        assert!(outcome.auto_policy_created);
        let policies = list_policies(&db).unwrap();
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].hostname, "*");
        match &policies[0].policy {
            TlsPolicy::AcmeDns { dns_provider } => assert_eq!(dns_provider, "primary"),
        }
    }

    #[test]
    fn second_provider_upsert_does_not_overwrite_existing_star() {
        let (db, cipher) = fresh_db();
        upsert_dns_provider(
            &db,
            &cipher,
            "primary",
            DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();
        let outcome = upsert_dns_provider(
            &db,
            &cipher,
            "secondary",
            DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();
        assert!(!outcome.auto_policy_created);
        // The catch-all still points at the first provider; operators can
        // re-bind it explicitly via /tls/policies/set-acme-dns.
        let policies = list_policies(&db).unwrap();
        let star = policies.iter().find(|p| p.hostname == "*").unwrap();
        match &star.policy {
            TlsPolicy::AcmeDns { dns_provider } => assert_eq!(dns_provider, "primary"),
        }
    }

    #[test]
    fn upsert_does_not_auto_create_star_when_policy_already_exists() {
        let (db, _cipher) = fresh_db();
        // Operator manually pinned a `*` policy before any provider was
        // configured (e.g. to a manual cert) — but that's impossible
        // because manual requires a cert_id; instead simulate having an
        // exact-match policy and confirm we still don't add `*`.
        // For this case we use a manual policy on an exact hostname so the
        // "providers existed before" branch can be exercised separately.
        // Catch-all by direct INSERT (bypassing the API) is approximated
        // here by inserting another provider first.
        let cipher = Cipher::for_tests();
        upsert_dns_provider(
            &db,
            &cipher,
            "first",
            DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();
        // Now pretend the operator cleared the auto-created `*` and
        // installed a different manual catch-all; verify a third provider
        // upsert leaves it alone.
        clear_policy(&db, "*").unwrap();
        db.conn
            .execute(
                "INSERT INTO tls_policies (hostname, strategy, dns_provider, cert_id, updated_at)
                 VALUES ('*', 'acme_dns', 'first', NULL, ?1)",
                params![now_secs()],
            )
            .unwrap();
        let outcome = upsert_dns_provider(
            &db,
            &cipher,
            "second",
            DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();
        assert!(!outcome.auto_policy_created);
    }

    // r[verify tls.policy.wildcard]
    #[test]
    fn resolve_policy_prefers_exact_over_wildcard() {
        let (db, cipher) = fresh_db();
        upsert_dns_provider(
            &db,
            &cipher,
            "p",
            DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();
        // The auto-created `*` is in place; add an exact + a `*.example.com`.
        set_policy_acme_dns(&db, "foo.example.com", "p").unwrap();
        set_policy_acme_dns(&db, "*.example.com", "p").unwrap();

        // Exact match wins.
        let row = resolve_policy(&db, "foo.example.com").unwrap().unwrap();
        assert_eq!(row.hostname, "foo.example.com");

        // No exact: dotted wildcard wins.
        let row = resolve_policy(&db, "baz.example.com").unwrap().unwrap();
        assert_eq!(row.hostname, "*.example.com");

        // Outside the dotted wildcard's suffix: catch-all wins.
        let row = resolve_policy(&db, "outside.org").unwrap().unwrap();
        assert_eq!(row.hostname, "*");
    }

    #[test]
    fn resolve_policy_returns_none_when_nothing_matches() {
        let (db, _cipher) = fresh_db();
        // No providers, no policies — every hostname uses the runtime default.
        assert!(
            resolve_policy(&db, "anything.example.com")
                .unwrap()
                .is_none()
        );
    }

    // r[verify tls.cert.attempt-log]
    #[test]
    fn attempt_lifecycle_round_trips() {
        let (db, _) = fresh_db();
        let id = insert_attempt(&db, "host.example.com", AttemptTrigger::OnDemand).unwrap();
        let rows = list_attempts(&db, Some("host.example.com"), 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].outcome, AttemptOutcome::Pending);
        assert!(rows[0].finished_at.is_none());

        finalize_attempt(
            &db,
            id,
            AttemptOutcome::Failure,
            None,
            Some("dns provider error"),
        )
        .unwrap();
        let rows = list_attempts(&db, Some("host.example.com"), 10).unwrap();
        assert_eq!(rows[0].outcome, AttemptOutcome::Failure);
        assert!(rows[0].finished_at.is_some());
        assert_eq!(rows[0].error.as_deref(), Some("dns provider error"));
    }

    #[test]
    fn list_attempts_returns_newest_first_and_obeys_limit() {
        let (db, _) = fresh_db();
        let mut ids = Vec::new();
        for _ in 0..5 {
            let id = insert_attempt(&db, "h.example.com", AttemptTrigger::Manual).unwrap();
            finalize_attempt(&db, id, AttemptOutcome::Success, None, None).unwrap();
            ids.push(id);
        }
        let rows = list_attempts(&db, None, 3).unwrap();
        assert_eq!(rows.len(), 3);
        // Newest first: attempt id descending.
        assert_eq!(rows[0].id, ids[4]);
        assert_eq!(rows[1].id, ids[3]);
        assert_eq!(rows[2].id, ids[2]);
    }

    // r[verify tls.cert.retry-block]
    #[test]
    fn retry_block_set_check_clear() {
        let (db, _) = fresh_db();
        assert!(!is_retry_blocked(&db, "host.example.com").unwrap());

        set_retry_block(
            &db,
            "host.example.com",
            RetryBlockSource::Auto,
            Some("dns 5xx"),
        )
        .unwrap();
        assert!(is_retry_blocked(&db, "host.example.com").unwrap());

        let rows = list_retry_blocks(&db).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].set_by, RetryBlockSource::Auto);
        assert_eq!(rows[0].reason.as_deref(), Some("dns 5xx"));

        // Setting again with the operator source replaces in place.
        set_retry_block(
            &db,
            "host.example.com",
            RetryBlockSource::Operator,
            Some("paused for migration"),
        )
        .unwrap();
        let rows = list_retry_blocks(&db).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].set_by, RetryBlockSource::Operator);
        assert_eq!(rows[0].reason.as_deref(), Some("paused for migration"));

        assert!(clear_retry_block(&db, "host.example.com").unwrap());
        assert!(!is_retry_blocked(&db, "host.example.com").unwrap());
        assert!(!clear_retry_block(&db, "host.example.com").unwrap());
    }

    // r[verify tls.settings.contact-email]
    #[test]
    fn settings_default_is_empty_then_persists() {
        let (db, _cipher) = fresh_db();
        let s = get_settings(&db).unwrap();
        assert_eq!(s.contact_email, "");

        set_contact_email(&db, "ops@example.com").unwrap();
        let s = get_settings(&db).unwrap();
        assert_eq!(s.contact_email, "ops@example.com");
        assert!(s.updated_at > 0);
    }
}
