//! Fetches TLS certificates for tailnet hostnames from the host's local
//! Tailscale facility and persists them in the runtime cert store, alongside
//! ACME-issued certs. The local API endpoint is
//! `GET /localapi/v0/cert/<hostname>?type=pair`, which returns a PEM
//! response containing the cert chain followed by the private key.
//!
//! This module is the building block; the wire-up that makes the issuance
//! Coordinator dispatch tailnet hostnames through here lands separately.
//
// r[impl ingress.site.tailscale]

use std::{path::PathBuf, sync::Arc};

use snafu::{ResultExt, Snafu};

use crate::runtime::{
    db::DbHandle,
    secrets::Cipher,
    tailscale,
    tls::{KeyType, TlsCertOrigin, TlsCertState, parse, store},
};

#[derive(Debug, Snafu)]
pub enum IssueError {
    #[snafu(display("tailscaled unreachable: {message}"))]
    Unreachable { message: String },

    #[snafu(display("tailscaled API returned status {status}: {body}"))]
    Api { status: u16, body: String },

    #[snafu(display("could not parse cert/key PEM: {message}"))]
    Pem { message: String },

    #[snafu(display("could not parse cert chain: {source}"))]
    ParseCert { source: parse::Error },

    #[snafu(display("could not encrypt private key: {source}"))]
    Cipher {
        source: crate::runtime::secrets::Error,
    },

    #[snafu(display("storage error: {source}"))]
    Storage { source: rusqlite::Error },
}

pub type Result<T, E = IssueError> = std::result::Result<T, E>;

#[derive(Debug, Clone)]
pub struct Issued {
    pub cert_id: i64,
    pub not_after: Option<i64>,
}

/// Fetch a fresh `(cert chain, private key)` pair for `hostname` from the
/// host's tailscaled local API and persist it in `tls_certificates` with
/// `origin=tailscale`. Idempotent across calls — repeated invocations for
/// the same hostname produce a fresh row each time and the previous row is
/// marked superseded (matches the ACME path's behaviour).
pub async fn issue(
    db: &DbHandle,
    cipher: &Arc<Cipher>,
    hostname: &str,
    socket_path: Option<PathBuf>,
) -> Result<Issued> {
    let socket = socket_path.unwrap_or_else(|| PathBuf::from(tailscale::DEFAULT_SOCKET_PATH));

    let client = reqwest::Client::builder()
        .unix_socket(socket)
        .build()
        .map_err(|e| IssueError::Unreachable {
            message: format!("client build failed: {e}"),
        })?;

    let url = format!("http://local/localapi/v0/cert/{hostname}?type=pair");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| IssueError::Unreachable {
            message: e.to_string(),
        })?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(IssueError::Api {
            status: status.as_u16(),
            body,
        });
    }
    let body = resp.text().await.map_err(|e| IssueError::Unreachable {
        message: e.to_string(),
    })?;

    let (cert_pem, key_pem) = split_pem_pair(&body)?;
    persist(db, cipher, hostname, &cert_pem, &key_pem).await
}

/// Split tailscaled's concatenated PEM response into `(cert_chain, key)`.
/// The body looks like:
///
/// ```text
/// -----BEGIN CERTIFICATE-----
/// ...
/// -----END CERTIFICATE-----
/// -----BEGIN CERTIFICATE-----  (optional intermediates)
/// ...
/// -----END CERTIFICATE-----
/// -----BEGIN PRIVATE KEY-----  (or EC PRIVATE KEY etc)
/// ...
/// -----END PRIVATE KEY-----
/// ```
///
/// We split on the *last* `BEGIN .* PRIVATE KEY` block; everything before
/// is the cert chain, everything from that block onwards is the key.
pub(crate) fn split_pem_pair(body: &str) -> Result<(String, String)> {
    let key_marker_idx = body
        .rmatch_indices("-----BEGIN ")
        .find_map(|(i, _)| {
            let after = &body[i..];
            // Look for "-----BEGIN <something> PRIVATE KEY-----"
            if after
                .lines()
                .next()
                .is_some_and(|l| l.contains("PRIVATE KEY"))
            {
                Some(i)
            } else {
                None
            }
        })
        .ok_or_else(|| IssueError::Pem {
            message: "no PRIVATE KEY block found in tailscaled response".to_owned(),
        })?;
    let cert = body[..key_marker_idx].trim().to_owned();
    let key = body[key_marker_idx..].trim().to_owned();
    if cert.is_empty() {
        return Err(IssueError::Pem {
            message: "no certificate block before PRIVATE KEY in tailscaled response".to_owned(),
        });
    }
    Ok((cert, key))
}

async fn persist(
    db: &DbHandle,
    cipher: &Arc<Cipher>,
    hostname: &str,
    cert_pem: &str,
    key_pem: &str,
) -> Result<Issued> {
    let parsed = parse::parse_chain(cert_pem).context(ParseCertSnafu)?;
    let key_secret = secrecy::SecretString::from(key_pem.to_owned());
    let key_ct = cipher.encrypt(&key_secret).context(CipherSnafu)?;
    let metadata = parsed.metadata.clone();
    let not_after = metadata.not_after;
    let chain_pem = parsed.chain_pem.clone();
    let hostname_owned = hostname.to_owned();
    let cert_id = db
        .call(move |db_inner| {
            let id = store::insert_certificate(
                db_inner,
                &hostname_owned,
                TlsCertState::Active,
                TlsCertOrigin::Tailscale,
                Some(&chain_pem),
                None,
                &key_ct,
                KeyType::EcdsaP256,
                metadata,
                None,
                None,
            )?;
            store::supersede_other_active_for_hostname(db_inner, &hostname_owned, id)?;
            Ok::<_, rusqlite::Error>(id)
        })
        .context(StorageSnafu)?;
    Ok(Issued { cert_id, not_after })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_pem_pair_separates_cert_chain_from_key() {
        let body = concat!(
            "-----BEGIN CERTIFICATE-----\n",
            "AAA\n",
            "-----END CERTIFICATE-----\n",
            "-----BEGIN CERTIFICATE-----\n",
            "BBB\n",
            "-----END CERTIFICATE-----\n",
            "-----BEGIN PRIVATE KEY-----\n",
            "CCC\n",
            "-----END PRIVATE KEY-----\n",
        );
        let (cert, key) = split_pem_pair(body).unwrap();
        assert!(cert.contains("BEGIN CERTIFICATE"));
        assert!(cert.contains("BBB"));
        assert!(!cert.contains("PRIVATE KEY"));
        assert!(key.starts_with("-----BEGIN PRIVATE KEY-----"));
        assert!(key.contains("CCC"));
    }

    #[test]
    fn split_pem_pair_handles_ec_private_key_marker() {
        let body = concat!(
            "-----BEGIN CERTIFICATE-----\n",
            "AAA\n",
            "-----END CERTIFICATE-----\n",
            "-----BEGIN EC PRIVATE KEY-----\n",
            "BBB\n",
            "-----END EC PRIVATE KEY-----\n",
        );
        let (cert, key) = split_pem_pair(body).unwrap();
        assert!(cert.contains("AAA"));
        assert!(key.contains("EC PRIVATE KEY"));
    }

    #[test]
    fn split_pem_pair_rejects_missing_key() {
        let body = "-----BEGIN CERTIFICATE-----\nAAA\n-----END CERTIFICATE-----\n";
        assert!(matches!(split_pem_pair(body), Err(IssueError::Pem { .. })));
    }

    #[test]
    fn split_pem_pair_rejects_missing_cert() {
        let body = "-----BEGIN PRIVATE KEY-----\nXXX\n-----END PRIVATE KEY-----\n";
        assert!(matches!(split_pem_pair(body), Err(IssueError::Pem { .. })));
    }
}
