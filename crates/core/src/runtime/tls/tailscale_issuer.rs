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

    // No redirect following: any 30x from tailscaled would otherwise
    // make reqwest re-issue with a Referer header set, which fails the
    // CSRF check at the localapi entrypoint on the next hop.
    let client = reqwest::Client::builder()
        .unix_socket(socket)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| IssueError::Unreachable {
            message: format!("client build failed: {e}"),
        })?;

    // tailscaled's local API rejects requests without the localapi
    // CSRF header (or a matching Host), so we send both.
    let url = format!("http://local-tailscaled.sock/localapi/v0/cert/{hostname}?type=pair");
    let resp = client
        .get(&url)
        .header("Sec-Tailscale", "localapi")
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
/// The body is one or more PEM blocks; tailscaled writes the private key
/// first followed by the leaf cert and any intermediates, but we don't
/// rely on order — every block whose label contains `PRIVATE KEY` goes
/// into the key bucket, every `CERTIFICATE` block goes into the chain
/// bucket, and we return them in chain-first / key form (the shape the
/// rest of the runtime expects).
pub(crate) fn split_pem_pair(body: &str) -> Result<(String, String)> {
    let mut chain_blocks: Vec<&str> = Vec::new();
    let mut key_blocks: Vec<&str> = Vec::new();
    let mut cursor = body;
    while let Some(begin_rel) = cursor.find("-----BEGIN ") {
        let block_start = &cursor[begin_rel..];
        // Pull the label off the first line so we know which bucket the
        // block belongs in (CERTIFICATE vs PRIVATE KEY).
        let header_line = block_start.lines().next().unwrap_or("");
        let label = header_line
            .strip_prefix("-----BEGIN ")
            .and_then(|s| s.strip_suffix("-----"))
            .unwrap_or("");
        // Match the corresponding `-----END <label>-----` to find the
        // block boundary. We accept any label that contains the matching
        // keyword so EC / RSA / PKCS8 private keys all flow through.
        let end_marker = format!("-----END {label}-----");
        let Some(end_rel) = block_start.find(&end_marker) else {
            return Err(IssueError::Pem {
                message: format!("no closing marker for {label:?} block"),
            });
        };
        let block_end = end_rel + end_marker.len();
        let block = &block_start[..block_end];
        if label.contains("PRIVATE KEY") {
            key_blocks.push(block);
        } else if label.contains("CERTIFICATE") {
            chain_blocks.push(block);
        }
        // Otherwise drop the block silently — tailscaled doesn't emit
        // anything else here, but if a future version starts including
        // OCSP staples or similar we don't want to choke.
        cursor = &block_start[block_end..];
    }
    if chain_blocks.is_empty() {
        return Err(IssueError::Pem {
            message: "no CERTIFICATE block in tailscaled response".to_owned(),
        });
    }
    if key_blocks.is_empty() {
        return Err(IssueError::Pem {
            message: "no PRIVATE KEY block in tailscaled response".to_owned(),
        });
    }
    Ok((chain_blocks.join("\n"), key_blocks.join("\n")))
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
    fn split_pem_pair_handles_key_first_then_cert() {
        // tailscaled emits the private key before the cert chain on the
        // /localapi/v0/cert/<host>?type=pair endpoint, so the parser
        // must not assume cert-then-key order.
        let body = concat!(
            "-----BEGIN EC PRIVATE KEY-----\n",
            "KKK\n",
            "-----END EC PRIVATE KEY-----\n",
            "-----BEGIN CERTIFICATE-----\n",
            "LEAF\n",
            "-----END CERTIFICATE-----\n",
            "-----BEGIN CERTIFICATE-----\n",
            "INTERMEDIATE\n",
            "-----END CERTIFICATE-----\n",
        );
        let (cert, key) = split_pem_pair(body).unwrap();
        assert!(cert.contains("LEAF"));
        assert!(cert.contains("INTERMEDIATE"));
        assert!(!cert.contains("PRIVATE KEY"));
        assert!(key.contains("EC PRIVATE KEY"));
        assert!(key.contains("KKK"));
        // Cert chain should appear leaf-first (the order tailscaled emits
        // them in, which is what the parse_chain consumer expects).
        let leaf_pos = cert.find("LEAF").unwrap();
        let intermediate_pos = cert.find("INTERMEDIATE").unwrap();
        assert!(leaf_pos < intermediate_pos);
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
