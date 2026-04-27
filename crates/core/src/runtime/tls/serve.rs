//! Certificate-serving module.
//!
//! Implements the runtime side of [`tls.cert.serve`](
//! ../../../../../docs/spec/runtime.md): on-demand lookup of a stored
//! certificate by SNI hostname, with the private key decrypted only at
//! request time and never written into the proxy's persistent
//! configuration.
//!
//! The lookup function is transport-agnostic. The daemon wires it behind a
//! small HTTP server bound to a local address that Caddy's
//! `tls.certificates.get_certificate.http` module fetches at handshake
//! time. The wire format is a single response body containing the leaf
//! cert chain followed by the private key, all as PEM blocks — that's
//! the convention Caddy expects.

use secrecy::{ExposeSecret, SecretString};
use snafu::{ResultExt, Snafu};

use super::{TlsCertOrigin, TlsCertState, store};
use crate::runtime::{db::DbHandle, secrets::Cipher};

#[derive(Debug, Snafu)]
pub enum ServeError {
    #[snafu(display("storage error: {source}"))]
    Storage { source: rusqlite::Error },

    #[snafu(display("decryption error: {source}"))]
    Cipher {
        source: crate::runtime::secrets::Error,
    },

    #[snafu(display("certificate row {id} has no PEM chain stored"))]
    MissingPem { id: i64 },
}

pub type Result<T, E = ServeError> = std::result::Result<T, E>;

/// A certificate ready for the proxy to serve.
pub struct CertBundle {
    pub cert_id: i64,
    pub origin: TlsCertOrigin,
    pub chain_pem: String,
    pub key_pem: SecretString,
}

/// Look up the active runtime-managed certificate for `hostname`.
///
/// Returns `Ok(None)` when there is no runtime-managed cert for the
/// hostname — the caller should respond 404 to Caddy, which falls back to
/// the default automation policy (ACME HTTP-01).
// r[impl tls.cert.serve]
pub async fn lookup(db: &DbHandle, cipher: &Cipher, hostname: &str) -> Result<Option<CertBundle>> {
    let hostname_owned = hostname.to_owned();
    let row_opt = db
        .call(move |db_inner| store::find_active_for_hostname(db_inner, &hostname_owned))
        .context(StorageSnafu)?;

    let Some(row) = row_opt else {
        return Ok(None);
    };

    if row.state != TlsCertState::Active {
        return Ok(None);
    }

    let Some(chain_pem) = row.cert_pem else {
        return MissingPemSnafu { id: row.id }.fail();
    };

    let key_pem = cipher.decrypt(&row.key_ciphertext).context(CipherSnafu)?;

    Ok(Some(CertBundle {
        cert_id: row.id,
        origin: row.origin,
        chain_pem,
        key_pem,
    }))
}

/// Format a [`CertBundle`] as Caddy's `tls.certificates.get_certificate.http`
/// expects: PEM cert chain followed by the PEM private key. A trailing
/// newline ensures both blocks are well-separated.
pub fn format_response(bundle: &CertBundle) -> String {
    let mut out = String::with_capacity(bundle.chain_pem.len() + 256);
    out.push_str(&bundle.chain_pem);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(bundle.key_pem.expose_secret());
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::db::Db;
    use crate::runtime::tls::{
        KeyType, TlsCertOrigin, TlsCertState,
        store::{CertMetadata, insert_certificate},
    };
    use secrecy::SecretString;

    async fn fresh() -> (DbHandle, Cipher) {
        let db = DbHandle::open_in_memory().unwrap();
        let cipher = Cipher::for_tests();
        (db, cipher)
    }

    fn insert_active(db: &DbHandle, cipher: &Cipher, hostname: &str, key_pem: &str) -> i64 {
        let host = hostname.to_owned();
        let key_ct = cipher
            .encrypt(&SecretString::new(key_pem.to_owned().into()))
            .unwrap();
        db.call(move |db_inner: &Db| -> i64 {
            insert_certificate(
                db_inner,
                &host,
                TlsCertState::Active,
                TlsCertOrigin::Manual,
                Some("-----BEGIN CERTIFICATE-----\nMIIBcert\n-----END CERTIFICATE-----\n"),
                None,
                &key_ct,
                KeyType::EcdsaP256,
                CertMetadata {
                    issuer: Some("CN=Test".to_owned()),
                    not_before: Some(1_700_000_000),
                    not_after: Some(1_800_000_000),
                    serial: Some("01".to_owned()),
                    self_signed: false,
                },
                None,
                None,
            )
            .unwrap()
        })
    }

    #[tokio::test]
    async fn lookup_returns_active_cert() {
        let (db, cipher) = fresh().await;
        let key_pem = "-----BEGIN PRIVATE KEY-----\nMIGdummykey\n-----END PRIVATE KEY-----\n";
        let id = insert_active(&db, &cipher, "foo.example.com", key_pem);

        let bundle = lookup(&db, &cipher, "foo.example.com")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bundle.cert_id, id);
        assert_eq!(bundle.origin, TlsCertOrigin::Manual);
        assert!(bundle.chain_pem.contains("BEGIN CERTIFICATE"));
        assert_eq!(bundle.key_pem.expose_secret(), key_pem);
    }

    #[tokio::test]
    async fn lookup_returns_none_when_no_cert() {
        let (db, cipher) = fresh().await;
        let result = lookup(&db, &cipher, "missing.example.com").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn format_response_concatenates_chain_then_key() {
        let bundle = CertBundle {
            cert_id: 1,
            origin: TlsCertOrigin::Manual,
            chain_pem: "-----BEGIN CERTIFICATE-----\ncertdata\n-----END CERTIFICATE-----\n"
                .to_owned(),
            key_pem: SecretString::new(
                "-----BEGIN PRIVATE KEY-----\nkeydata\n-----END PRIVATE KEY-----\n".into(),
            ),
        };
        let body = format_response(&bundle);
        let cert_pos = body.find("BEGIN CERTIFICATE").unwrap();
        let key_pos = body.find("BEGIN PRIVATE KEY").unwrap();
        assert!(cert_pos < key_pos, "cert must precede key");
    }

    #[tokio::test]
    async fn format_response_inserts_separator_when_missing() {
        let bundle = CertBundle {
            cert_id: 1,
            origin: TlsCertOrigin::Manual,
            chain_pem: "-----BEGIN CERTIFICATE-----\nCERT\n-----END CERTIFICATE-----".to_owned(),
            key_pem: SecretString::new(
                "-----BEGIN PRIVATE KEY-----\nKEY\n-----END PRIVATE KEY-----".into(),
            ),
        };
        let body = format_response(&bundle);
        assert!(body.contains("-----END CERTIFICATE-----\n-----BEGIN PRIVATE KEY-----"));
        assert!(body.ends_with('\n'));
    }
}
