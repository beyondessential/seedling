//! Database CRUD for TLS provider, certificate, policy, and ACME-account rows.
//!
//! All credential and key material passes through [`Cipher`] at the
//! storage boundary; in-memory representations carry plaintext only when
//! actively in use.

use jiff::Timestamp;
use rusqlite::{OptionalExtension, params};
use secrecy::{ExposeSecret, SecretString};

use super::{
    AcmeAccount, DnsProviderEntry, DnsProviderKind, DnsProviderSummary, KeyType, TlsCertOrigin,
    TlsCertState, TlsCertificate, TlsPolicy, TlsPolicyRow,
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

// r[impl tls.dns-provider.lifecycle]
pub fn upsert_dns_provider(
    db: &Db,
    cipher: &Cipher,
    name: &str,
    kind: DnsProviderKind,
    config: &SecretString,
) -> rusqlite::Result<()> {
    let ct = cipher
        .encrypt(config)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let now = now_secs();
    db.conn.execute(
        "INSERT INTO tls_dns_providers (name, kind, config_ciphertext, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(name) DO UPDATE SET
             kind = excluded.kind,
             config_ciphertext = excluded.config_ciphertext,
             updated_at = excluded.updated_at",
        params![name, kind.as_str(), ct, now],
    )?;
    Ok(())
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
            let cert_id: Option<i64> = row.get(3)?;
            let updated_at: i64 = row.get(4)?;
            let policy = match strategy.as_str() {
                "acme_dns" => TlsPolicy::AcmeDns {
                    dns_provider: dns_provider.ok_or_else(|| rusqlite::Error::InvalidQuery)?,
                },
                "manual" => TlsPolicy::Manual {
                    cert_id: cert_id.ok_or_else(|| rusqlite::Error::InvalidQuery)?,
                },
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            Ok(TlsPolicyRow {
                hostname,
                policy,
                updated_at,
            })
        })?
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

// r[impl tls.strategy.manual]
// r[impl tls.policy.apply]
pub fn set_policy_manual(db: &Db, hostname: &str, cert_id: i64) -> rusqlite::Result<()> {
    let now = now_secs();
    db.conn.execute(
        "INSERT INTO tls_policies (hostname, strategy, dns_provider, cert_id, updated_at)
         VALUES (?1, 'manual', NULL, ?2, ?3)
         ON CONFLICT(hostname) DO UPDATE SET
             strategy = excluded.strategy,
             dns_provider = NULL,
             cert_id = excluded.cert_id,
             updated_at = excluded.updated_at",
        params![hostname, cert_id, now],
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
                    note, acme_account_id, created_at, updated_at
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
                note, acme_account_id, created_at, updated_at
         FROM tls_certificates ORDER BY id DESC",
    )?;
    stmt.query_map([], row_to_certificate)?.collect()
}

/// Returns the most-recent active cert for a hostname, if any.
pub fn find_active_for_hostname(
    db: &Db,
    hostname: &str,
) -> rusqlite::Result<Option<TlsCertificate>> {
    db.conn
        .query_row(
            "SELECT id, hostname, state, origin, cert_pem, csr_pem, key_ciphertext,
                    key_type, issuer, not_before, not_after, serial, self_signed,
                    note, acme_account_id, created_at, updated_at
             FROM tls_certificates
             WHERE hostname = ?1 AND state = 'active'
             ORDER BY id DESC LIMIT 1",
            [hostname],
            row_to_certificate,
        )
        .optional()
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
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
    })
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
        set_policy_acme_dns(&db, "foo.example.com", "p").unwrap();
        set_policy_acme_dns(&db, "bar.example.com", "p").unwrap();

        let rows = list_policies(&db).unwrap();
        assert_eq!(rows.len(), 2);
        match &rows[0].policy {
            TlsPolicy::AcmeDns { dns_provider } => assert_eq!(dns_provider, "p"),
            _ => panic!("expected acme_dns"),
        }
    }

    #[test]
    fn policy_manual_then_clear() {
        let (db, _) = fresh_db();
        let cert_id = insert_test_cert(&db, "foo.example.com");
        set_policy_manual(&db, "foo.example.com", cert_id).unwrap();

        let row = get_policy(&db, "foo.example.com").unwrap().unwrap();
        match row.policy {
            TlsPolicy::Manual { cert_id: c } => assert_eq!(c, cert_id),
            _ => panic!("expected manual"),
        }

        assert!(clear_policy(&db, "foo.example.com").unwrap());
        assert!(get_policy(&db, "foo.example.com").unwrap().is_none());
    }

    #[test]
    fn policy_strategy_swap_keeps_single_row() {
        let (db, cipher) = fresh_db();
        upsert_dns_provider(
            &db,
            &cipher,
            "p",
            DnsProviderKind::Route53,
            &provider_config(),
        )
        .unwrap();
        let cert_id = insert_test_cert(&db, "foo.example.com");

        set_policy_acme_dns(&db, "foo.example.com", "p").unwrap();
        set_policy_manual(&db, "foo.example.com", cert_id).unwrap();

        let rows = list_policies(&db).unwrap();
        assert_eq!(rows.len(), 1);
        match &rows[0].policy {
            TlsPolicy::Manual { cert_id: c } => assert_eq!(*c, cert_id),
            _ => panic!("expected manual after swap"),
        }
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
}
