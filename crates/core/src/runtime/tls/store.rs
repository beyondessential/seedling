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
                // r[impl ingress.site.tailscale]
                "tailscale" => TlsPolicy::Tailscale,
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

/// Every column [`row_to_certificate`] reads, in the order it reads them.
/// Written once so a new column cannot be added to some of the four
/// certificate queries and forgotten in the others, which would shift the
/// positional indices the reader depends on.
const CERT_COLUMNS: &str = "id, hostname, requested_hostname, state, origin, cert_pem, csr_pem, \
     key_ciphertext, key_type, issuer, not_before, not_after, serial, self_signed, note, \
     acme_account_id, ari_window_start, ari_window_end, ari_polled_at, created_at, updated_at";

/// The fields of a new certificate row. A struct rather than a parameter list
/// because `hostname` and `requested_hostname` are both bare hostnames sitting
/// next to each other: passed positionally they are exactly the pair a caller
/// would swap, and telling them apart is the whole point of the second one.
pub struct NewCertificate<'a> {
    /// The row's label, which must be a name the certificate covers; see
    /// [`TlsCertificate::hostname`]. A row inserted before its certificate
    /// exists — a pending CSR — labels itself with the requested name until the
    /// upload replaces it.
    pub hostname: &'a str,
    /// The hostname a CSR was requested for; see
    /// [`TlsCertificate::requested_hostname`]. `None` for every other origin.
    pub requested_hostname: Option<&'a str>,
    pub state: TlsCertState,
    pub origin: TlsCertOrigin,
    pub cert_pem: Option<&'a str>,
    pub csr_pem: Option<&'a str>,
    pub key_ciphertext: &'a [u8],
    pub key_type: KeyType,
    pub metadata: CertMetadata,
    pub note: Option<&'a str>,
    pub acme_account_id: Option<i64>,
}

pub fn insert_certificate(db: &Db, new: NewCertificate<'_>) -> rusqlite::Result<i64> {
    let now = now_secs();
    db.conn.execute(
        "INSERT INTO tls_certificates (
            hostname, requested_hostname, state, origin, cert_pem, csr_pem,
            key_ciphertext, key_type, issuer, not_before, not_after, serial,
            self_signed, note, acme_account_id, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16)",
        params![
            new.hostname,
            new.requested_hostname,
            new.state.as_str(),
            new.origin.as_str(),
            new.cert_pem,
            new.csr_pem,
            new.key_ciphertext,
            new.key_type.as_str(),
            new.metadata.issuer,
            new.metadata.not_before,
            new.metadata.not_after,
            new.metadata.serial,
            new.metadata.self_signed as i64,
            new.note,
            new.acme_account_id,
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
            &format!("SELECT {CERT_COLUMNS} FROM tls_certificates WHERE id = ?1"),
            [id],
            row_to_certificate,
        )
        .optional()
}

pub fn list_certificates(db: &Db) -> rusqlite::Result<Vec<TlsCertificate>> {
    let mut stmt = db.conn.prepare(&format!(
        "SELECT {CERT_COLUMNS} FROM tls_certificates ORDER BY id DESC"
    ))?;
    stmt.query_map([], row_to_certificate)?.collect()
}

/// Returns the most-recent unexpired active cert covering `hostname`, if any.
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
///
/// Both paths skip rows whose `not_after` has passed. Without that the
/// exact-hostname fast path returned an expired row in preference to a
/// still-valid cert that covered the same hostname by SAN, so the proxy
/// served the expired one and nothing on the serve path noticed. A row with
/// no recorded `not_after` is kept: unparsed and expired are not the same
/// thing, and treating them alike would stop serving a usable cert.
///
/// The sibling matcher in [`super::state`] deliberately does *not* filter on
/// expiry — the renewal scheduler has to see an expiring cert in order to
/// renew it — so the two are not interchangeable despite the shared rules.
// r[impl tls.strategy.manual]
// r[impl tls.cert.serve]
pub fn find_active_for_hostname(
    db: &Db,
    hostname: &str,
) -> rusqlite::Result<Option<TlsCertificate>> {
    let now = now_secs();

    // The label is an index hint, not an answer. A row whose label says one
    // thing while its certificate covers another would otherwise be handed out
    // for a name it cannot serve — a TLS name mismatch — in preference to the
    // certificate that does cover it. Rows written before the label was made
    // to follow the certificate can be exactly that, so the hint is confirmed
    // against the certificate before it is trusted, and falls through to the
    // scan when it does not hold.
    // r[impl tls.cert.validation.san-coverage]
    if let Some(cert) = db
        .conn
        .query_row(
            &format!(
                "SELECT {CERT_COLUMNS}
                 FROM tls_certificates
                 WHERE hostname = ?1 AND state = 'active'
                   AND (not_before IS NULL OR not_before <= ?2)
                   AND (not_after IS NULL OR not_after > ?2)
                 ORDER BY id DESC LIMIT 1"
            ),
            rusqlite::params![hostname, now],
            row_to_certificate,
        )
        .optional()?
        && cert
            .cert_pem
            .as_deref()
            .is_some_and(|pem| super::parse::cert_covers(pem, hostname).unwrap_or(false))
    {
        return Ok(Some(cert));
    }

    // SAN-coverage scan: walk active certs newest-first and return the
    // first whose SAN list covers the hostname. Cost is one PEM parse
    // per active row; in operator-scale databases (<<1000 active rows)
    // this is microseconds.
    let mut stmt = db.conn.prepare(&format!(
        "SELECT {CERT_COLUMNS}
             FROM tls_certificates
             WHERE state = 'active'
               AND (not_before IS NULL OR not_before <= ?1)
           AND (not_after IS NULL OR not_after > ?1)
             ORDER BY created_at DESC, id DESC"
    ))?;
    let mut rows = stmt.query([now])?;
    while let Some(row) = rows.next()? {
        let cert = row_to_certificate(row)?;
        let Some(pem) = cert.cert_pem.as_deref() else {
            continue;
        };
        if super::parse::cert_covers(pem, hostname).unwrap_or(false) {
            return Ok(Some(cert));
        }
    }
    Ok(None)
}

/// Transition a cert to a new state, optionally updating its primary-SAN
/// label, cert PEM, and parsed metadata. Used by:
///
/// - CSR upload: pending → active, supplying the label the signed certificate
///   turned out to carry, plus cert_pem + parsed metadata.
/// - ACME renewal: active → superseded for the old row.
///
/// `hostname` is the row's primary SAN, so a caller that supplies a new
/// certificate supplies the label from that certificate rather than leaving
/// a label the new certificate may not cover.
// r[impl tls.cert.validation.san-coverage]
pub fn update_certificate(
    db: &Db,
    id: i64,
    hostname: Option<&str>,
    state: TlsCertState,
    cert_pem: Option<&str>,
    metadata: Option<&CertMetadata>,
) -> rusqlite::Result<()> {
    let now = now_secs();
    db.conn.execute(
        "UPDATE tls_certificates SET
            hostname = COALESCE(?1, hostname),
            state = ?2,
            cert_pem = COALESCE(?3, cert_pem),
            issuer = COALESCE(?4, issuer),
            not_before = COALESCE(?5, not_before),
            not_after = COALESCE(?6, not_after),
            serial = COALESCE(?7, serial),
            self_signed = COALESCE(?8, self_signed),
            updated_at = ?9
         WHERE id = ?10",
        params![
            hostname,
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

/// Retire the active certificates that the certificate `keep_id` replaces.
///
/// Replacement is a strict improvement or it does not happen. A candidate is
/// retired only when the arriving certificate is
///
/// - **at least as broad**: it covers every name the candidate serves. Sharing
///   a label says nothing about the rest of a candidate's SAN set — a
///   candidate carrying `[example.com, shop.example.com]` shares its label with
///   an arriving `[example.com, www.example.com]` while serving a name the
///   arriving certificate cannot, and retiring it would leave
///   `shop.example.com` with no active certificate at all.
/// - **at least as serviceable**: the arriving certificate is inside its own
///   validity window, and is not self-signed unless the candidate already was.
///   A certificate staged ahead of its `notBefore` is accepted deliberately
///   (see `tls.cert.validation.expired`) and a self-signed one is accepted with
///   an annotation; neither is grounds for retiring a certificate clients
///   currently accept. This matters most on the CSR path, where the SAN set —
///   and so which certificates become candidates at all — is chosen by the
///   external CA rather than by the operator.
///
/// Candidates are the other active rows sharing the arriving certificate's
/// label, which is how a renewal finds its predecessor. A candidate whose
/// certificate cannot be read is left alone: being unable to tell what it
/// serves is not the same as knowing the arriving certificate replaces it.
///
/// Returns the number of rows retired.
// r[impl tls.cert.validation.san-coverage]
pub fn supersede_other_active_for_hostname(
    db: &Db,
    hostname: &str,
    keep_id: i64,
) -> rusqlite::Result<usize> {
    // Read the arriving certificate back from the row rather than taking its
    // properties as arguments, so what is compared is what was actually
    // stored.
    let Some(arriving) = get_certificate(db, keep_id)? else {
        return Ok(0);
    };
    let Some(arriving_sans) = arriving
        .cert_pem
        .as_deref()
        .and_then(|pem| super::parse::leaf_san_dns_names(pem).ok())
    else {
        return Ok(0);
    };

    let now = now_secs();
    if arriving.not_before.is_some_and(|nb| nb > now)
        || arriving.not_after.is_some_and(|na| na <= now)
    {
        return Ok(0);
    }

    let candidates: Vec<TlsCertificate> = {
        let mut stmt = db.conn.prepare(&format!(
            "SELECT {CERT_COLUMNS}
             FROM tls_certificates
             WHERE hostname = ?1 AND state = 'active' AND id != ?2"
        ))?;
        stmt.query_map(params![hostname, keep_id], row_to_certificate)?
            .collect::<rusqlite::Result<_>>()?
    };

    let mut retired = 0;
    for candidate in candidates {
        if arriving.self_signed && !candidate.self_signed {
            continue;
        }
        let Some(pem) = candidate.cert_pem.as_deref() else {
            continue;
        };
        let Ok(served) = super::parse::leaf_san_dns_names(pem) else {
            continue;
        };
        if !served
            .iter()
            .all(|name| super::parse::san_covers(&arriving_sans, name))
        {
            continue;
        }
        retired += db.conn.execute(
            "UPDATE tls_certificates SET state = 'superseded', updated_at = ?1 WHERE id = ?2",
            params![now, candidate.id],
        )?;
    }
    Ok(retired)
}

pub fn delete_certificate(db: &Db, id: i64) -> rusqlite::Result<bool> {
    let n = db
        .conn
        .execute("DELETE FROM tls_certificates WHERE id = ?1", [id])?;
    Ok(n > 0)
}

/// Reads the columns of [`CERT_COLUMNS`], positionally and in that order.
fn row_to_certificate(row: &rusqlite::Row<'_>) -> rusqlite::Result<TlsCertificate> {
    let state_str: String = row.get(3)?;
    let origin_str: String = row.get(4)?;
    let key_type_str: String = row.get(8)?;
    let self_signed_int: i64 = row.get(13)?;
    Ok(TlsCertificate {
        id: row.get(0)?,
        hostname: row.get(1)?,
        requested_hostname: row.get(2)?,
        state: TlsCertState::parse(&state_str).ok_or(rusqlite::Error::InvalidQuery)?,
        origin: TlsCertOrigin::parse(&origin_str).ok_or(rusqlite::Error::InvalidQuery)?,
        cert_pem: row.get(5)?,
        csr_pem: row.get(6)?,
        key_ciphertext: row.get(7)?,
        key_type: KeyType::parse(&key_type_str).ok_or(rusqlite::Error::InvalidQuery)?,
        issuer: row.get(9)?,
        not_before: row.get(10)?,
        not_after: row.get(11)?,
        serial: row.get(12)?,
        self_signed: self_signed_int != 0,
        note: row.get(14)?,
        acme_account_id: row.get(15)?,
        ari_window_start: row.get(16)?,
        ari_window_end: row.get(17)?,
        ari_polled_at: row.get(18)?,
        created_at: row.get(19)?,
        updated_at: row.get(20)?,
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

// r[impl tls.acme.account.persist]
/// Look up the persisted account for an ACME directory regardless of the
/// stored contact email. Used by the load-or-create path so that a
/// changed contact email reuses the same account (with a follow-up
/// `update_contacts` call) instead of registering a fresh one.
///
/// If a database somehow holds more than one row for the same directory
/// (a leftover from when the schema keyed on `(directory, email)` and
/// the email was changed), the most-recently-updated row wins.
pub fn get_acme_account_for_directory(
    db: &Db,
    directory_url: &str,
) -> rusqlite::Result<Option<AcmeAccount>> {
    db.conn
        .query_row(
            "SELECT id, directory_url, contact_email, account_url,
                    account_key_ciphertext, created_at, updated_at
             FROM tls_acme_accounts
             WHERE directory_url = ?1
             ORDER BY updated_at DESC, id DESC
             LIMIT 1",
            params![directory_url],
            row_to_acme_account,
        )
        .optional()
}

// r[impl tls.acme.account.contact-update]
/// Update the persisted contact email for an existing account row, after
/// the directory has accepted a `update_contacts` call.
pub fn set_acme_account_contact_email(
    db: &Db,
    id: i64,
    contact_email: &str,
) -> rusqlite::Result<()> {
    let now = now_secs();
    db.conn.execute(
        "UPDATE tls_acme_accounts
         SET contact_email = ?1, updated_at = ?2
         WHERE id = ?3",
        params![contact_email, now, id],
    )?;
    Ok(())
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

    /// A real self-signed certificate carrying exactly `sans`, so the coverage
    /// checks have something they can parse.
    fn real_cert_pem(sans: &[&str]) -> String {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("keypair");
        let mut params =
            rcgen::CertificateParams::new(sans.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
                .expect("params");
        params.distinguished_name = rcgen::DistinguishedName::new();
        params.self_signed(&key).expect("self-sign").pem()
    }

    fn insert_test_cert(db: &Db, hostname: &str) -> i64 {
        insert_certificate(
            db,
            NewCertificate {
                hostname,
                requested_hostname: None,
                state: TlsCertState::Active,
                origin: TlsCertOrigin::Manual,
                cert_pem: Some(&real_cert_pem(&[hostname])),
                csr_pem: None,
                key_ciphertext: b"encrypted-key-bytes",
                key_type: KeyType::EcdsaP256,
                metadata: CertMetadata {
                    issuer: Some("CN=Test CA".to_string()),
                    not_before: Some(1_700_000_000),
                    not_after: Some(1_800_000_000),
                    serial: Some("01".to_string()),
                    self_signed: false,
                },
                note: None,
                acme_account_id: None,
            },
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

    /// Like [`insert_test_cert`] but with an explicit expiry, so a row can be
    /// placed either side of "now".
    fn insert_test_cert_expiring(db: &Db, hostname: &str, not_after: Option<i64>) -> i64 {
        insert_certificate(
            db,
            NewCertificate {
                hostname,
                requested_hostname: None,
                state: TlsCertState::Active,
                origin: TlsCertOrigin::Manual,
                cert_pem: Some(&real_cert_pem(&[hostname])),
                csr_pem: None,
                key_ciphertext: b"encrypted-key-bytes",
                key_type: KeyType::EcdsaP256,
                metadata: CertMetadata {
                    issuer: Some("CN=Test CA".to_string()),
                    not_before: Some(1_600_000_000),
                    not_after,
                    serial: Some("01".to_string()),
                    self_signed: false,
                },
                note: None,
                acme_account_id: None,
            },
        )
        .unwrap()
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

    /// A row labelled `label` whose certificate carries exactly `sans`. The two
    /// are separate arguments so a test can build the mislabelled row this
    /// subsystem is meant to make impossible.
    fn insert_labelled_cert(db: &Db, label: &str, sans: &[&str]) -> i64 {
        insert_cert_with(db, label, sans, |_| {})
    }

    fn insert_cert_with(
        db: &Db,
        label: &str,
        sans: &[&str],
        tweak: impl FnOnce(&mut CertMetadata),
    ) -> i64 {
        let mut metadata = CertMetadata {
            not_after: Some(now_secs() + 86400),
            ..Default::default()
        };
        tweak(&mut metadata);
        insert_certificate(
            db,
            NewCertificate {
                hostname: label,
                requested_hostname: None,
                state: TlsCertState::Active,
                origin: TlsCertOrigin::Manual,
                cert_pem: Some(&real_cert_pem(sans)),
                csr_pem: None,
                key_ciphertext: b"key",
                key_type: KeyType::EcdsaP256,
                metadata,
                note: None,
                acme_account_id: None,
            },
        )
        .unwrap()
    }

    /// On the CSR path the SAN set is chosen by the external CA, so which
    /// certificates become supersession candidates is outside the operator's
    /// control. A certificate staged ahead of its validity window is accepted
    /// deliberately, and must not retire one clients accept today.
    // r[verify tls.cert.validation.san-coverage]
    #[test]
    fn supersede_spares_an_incumbent_when_the_new_cert_is_not_yet_valid() {
        let (db, _) = fresh_db();
        let incumbent = insert_labelled_cert(&db, "example.com", &["example.com"]);
        let staged = insert_cert_with(&db, "example.com", &["example.com"], |m| {
            m.not_before = Some(now_secs() + 86_400);
            m.not_after = Some(now_secs() + 90 * 86_400);
        });

        assert_eq!(
            supersede_other_active_for_hostname(&db, "example.com", staged).unwrap(),
            0
        );
        assert_eq!(
            get_certificate(&db, incumbent).unwrap().unwrap().state,
            TlsCertState::Active
        );
    }

    /// Nor may a self-signed certificate retire one a CA issued: clients accept
    /// the incumbent and would reject the replacement.
    // r[verify tls.cert.validation.san-coverage]
    #[test]
    fn supersede_spares_a_ca_issued_incumbent_when_the_new_cert_is_self_signed() {
        let (db, _) = fresh_db();
        let incumbent = insert_labelled_cert(&db, "example.com", &["example.com"]);
        let arriving = insert_cert_with(&db, "example.com", &["example.com"], |m| {
            m.self_signed = true;
        });

        assert_eq!(
            supersede_other_active_for_hostname(&db, "example.com", arriving).unwrap(),
            0
        );
        assert_eq!(
            get_certificate(&db, incumbent).unwrap().unwrap().state,
            TlsCertState::Active
        );
    }

    /// A certificate staged ahead of its `notBefore` is stored deliberately, so
    /// it must wait rather than be served and rejected by clients.
    // r[verify tls.cert.serve]
    #[test]
    fn a_not_yet_valid_cert_is_not_served() {
        let (db, _) = fresh_db();
        insert_cert_with(&db, "example.com", &["example.com"], |m| {
            m.not_before = Some(now_secs() + 86_400);
            m.not_after = Some(now_secs() + 90 * 86_400);
        });

        assert!(
            find_active_for_hostname(&db, "example.com")
                .unwrap()
                .is_none(),
            "a cert outside its validity window must not be served",
        );
    }

    /// A row written before the label was made to follow the certificate: it
    /// claims `www.example.com` while carrying a certificate for
    /// `example.com`. Serving it for the name on the label would be a TLS name
    /// mismatch, so the lookup must not prefer it to the certificate that does
    /// cover that name — nor return it when no other does.
    // r[verify tls.cert.validation.san-coverage]
    // r[verify tls.cert.serve]
    #[test]
    fn find_active_for_hostname_does_not_hand_out_a_non_covering_cert() {
        let (db, _) = fresh_db();
        let mislabelled = insert_labelled_cert(&db, "www.example.com", &["example.com"]);

        assert!(
            find_active_for_hostname(&db, "www.example.com")
                .unwrap()
                .is_none(),
            "a row labelled www.example.com whose cert covers only example.com \
             must not be served for www.example.com",
        );

        // And once a certificate that really does cover the name exists, that
        // is the one handed out, even though the mislabelled row is newer.
        let covering = insert_labelled_cert(&db, "example.com", &["www.example.com"]);
        let found = find_active_for_hostname(&db, "www.example.com")
            .unwrap()
            .expect("a certificate covers www.example.com");
        assert_eq!(found.id, covering);
        assert_ne!(found.id, mislabelled);
    }

    /// An arriving certificate replaces what it can serve in full, and nothing
    /// else. A candidate sharing its label may still carry names it does not
    /// cover; retiring that candidate would leave those names with no active
    /// certificate at all.
    // r[verify tls.cert.validation.san-coverage]
    #[test]
    fn supersede_spares_a_cert_serving_a_name_the_new_one_does_not_cover() {
        let (db, _) = fresh_db();
        let incumbent =
            insert_labelled_cert(&db, "example.com", &["example.com", "shop.example.com"]);
        let arriving =
            insert_labelled_cert(&db, "example.com", &["example.com", "www.example.com"]);

        let retired = supersede_other_active_for_hostname(&db, "example.com", arriving).unwrap();

        assert_eq!(retired, 0, "shop.example.com would have been stranded");
        assert_eq!(
            get_certificate(&db, incumbent).unwrap().unwrap().state,
            TlsCertState::Active
        );
        // ...and it is still what shop.example.com is served.
        assert_eq!(
            find_active_for_hostname(&db, "shop.example.com")
                .unwrap()
                .expect("shop.example.com still has a certificate")
                .id,
            incumbent
        );
    }

    /// The renewal case the supersession exists for: the arriving certificate
    /// covers everything the incumbent did, so the incumbent moves to history.
    // r[verify tls.cert.validation.san-coverage]
    #[test]
    fn supersede_retires_a_cert_the_new_one_fully_covers() {
        let (db, _) = fresh_db();
        let incumbent =
            insert_labelled_cert(&db, "example.com", &["example.com", "www.example.com"]);
        let arriving =
            insert_labelled_cert(&db, "example.com", &["example.com", "www.example.com"]);

        let retired = supersede_other_active_for_hostname(&db, "example.com", arriving).unwrap();

        assert_eq!(retired, 1);
        assert_eq!(
            get_certificate(&db, incumbent).unwrap().unwrap().state,
            TlsCertState::Superseded
        );
    }

    // r[verify tls.cert.serve]
    #[test]
    fn find_active_for_hostname_skips_an_expired_cert() {
        let (db, _) = fresh_db();
        insert_test_cert_expiring(&db, "a.example.com", Some(now_secs() - 1));

        assert!(
            find_active_for_hostname(&db, "a.example.com")
                .unwrap()
                .is_none(),
            "an expired cert must not be served",
        );
    }

    // r[verify tls.cert.serve]
    // The exact-hostname path takes the highest id, so a newer expired row
    // shadowed an older one that was still valid — and the proxy served the
    // expired cert with nothing on the path noticing.
    #[test]
    fn an_expired_cert_does_not_shadow_a_valid_one_for_the_same_hostname() {
        let (db, _) = fresh_db();
        let valid = insert_test_cert_expiring(&db, "a.example.com", Some(now_secs() + 86_400));
        let expired = insert_test_cert_expiring(&db, "a.example.com", Some(now_secs() - 1));
        assert!(expired > valid, "the expired row must be the newer one");

        let found = find_active_for_hostname(&db, "a.example.com")
            .unwrap()
            .expect("the valid cert is still servable");
        assert_eq!(found.id, valid);
    }

    // r[verify tls.cert.serve]
    // Unparsed and expired are different things; refusing to serve a row whose
    // expiry was never recorded would withhold a usable cert.
    #[test]
    fn a_cert_with_no_recorded_expiry_is_still_served() {
        let (db, _) = fresh_db();
        let id = insert_test_cert_expiring(&db, "a.example.com", None);

        let found = find_active_for_hostname(&db, "a.example.com")
            .unwrap()
            .expect("an unrecorded expiry is not an expiry");
        assert_eq!(found.id, id);
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
            NewCertificate {
                hostname: "www.example.com",
                requested_hostname: Some("www.example.com"),
                state: TlsCertState::CsrPending,
                origin: TlsCertOrigin::Csr,
                cert_pem: None,
                csr_pem: Some(
                    "-----BEGIN CERTIFICATE REQUEST-----\nMIICSR\n-----END CERTIFICATE REQUEST-----\n",
                ),
                key_ciphertext: b"key",
                key_type: KeyType::EcdsaP256,
                metadata: CertMetadata::default(),
                note: None,
                acme_account_id: None,
            },
        )
        .unwrap();

        // The CA signed a different name than the CSR asked for, so the row is
        // relabelled to what arrived while the request stays on the record.
        update_certificate(
            &db,
            id,
            Some("example.com"),
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
        assert_eq!(row.hostname, "example.com");
        assert_eq!(row.requested_hostname.as_deref(), Some("www.example.com"));
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
            other => panic!("expected AcmeDns policy, got {other:?}"),
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
            other => panic!("expected AcmeDns policy, got {other:?}"),
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

    // r[verify tls.acme.account.persist]
    // r[verify tls.acme.account.contact-update]
    #[test]
    fn acme_account_lookup_by_directory_and_contact_update() {
        let (db, cipher) = fresh_db();
        let key_pem = SecretString::new(
            "-----BEGIN PRIVATE KEY-----\ndummy\n-----END PRIVATE KEY-----\n".into(),
        );
        let id = insert_acme_account(
            &db,
            &cipher,
            "https://acme.example/directory",
            "old@example.com",
            "https://acme.example/acme/acct/1",
            &key_pem,
        )
        .unwrap();

        // Lookup by directory alone returns the account regardless of email.
        let by_dir = get_acme_account_for_directory(&db, "https://acme.example/directory")
            .unwrap()
            .unwrap();
        assert_eq!(by_dir.id, id);
        assert_eq!(by_dir.contact_email, "old@example.com");

        set_acme_account_contact_email(&db, id, "new@example.com").unwrap();
        let after = get_acme_account_for_directory(&db, "https://acme.example/directory")
            .unwrap()
            .unwrap();
        assert_eq!(after.contact_email, "new@example.com");
        assert_eq!(after.id, id, "row id is preserved across email updates");
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
            other => panic!("expected AcmeDns policy, got {other:?}"),
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
            other => panic!("expected AcmeDns policy, got {other:?}"),
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
