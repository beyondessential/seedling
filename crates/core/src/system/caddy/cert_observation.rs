// spec: OBS#ingresses
//
// Observe Caddy's on-disk certificate cache and emit `cert_valid` /
// `cert_acquisition_failed` observations against ingress instances.
//
// Caddy stores certs at `<caddy_data>/caddy/certificates/<issuer>/<host>/<host>.crt`,
// where `<issuer>` is e.g. `local` (internal CA) or
// `acme-v02.api.letsencrypt.org-directory`. We don't need to know the issuer
// in advance; we glob across all issuer subdirectories.

use std::path::Path;

use serde_json::{Value, json};

use crate::runtime::identity::ResourceInstance;

/// Returns whether a cert file exists for the given hostname under any issuer.
pub(crate) fn cert_present(caddy_data_path: &Path, hostname: &str) -> bool {
    matches!(cert_path_for(caddy_data_path, hostname), Ok(Some(_)))
}

/// Locate the on-disk cert file for `hostname` under any issuer subdir.
///
/// `Ok(None)` means the cache was read and holds nothing for the hostname. A
/// cache that could not be read is an error, since that cannot be told from
/// "holds nothing" by looking at the result.
fn cert_path_for(
    caddy_data_path: &Path,
    hostname: &str,
) -> std::io::Result<Option<(String, std::path::PathBuf)>> {
    let cert_root = caddy_data_path
        .join(super::image::CERT_CACHE_DIR)
        .join("certificates");
    let issuers = match std::fs::read_dir(&cert_root) {
        Ok(issuers) => issuers,
        // The proxy has not written a certificate yet.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    for entry in issuers {
        let entry = entry?;
        let issuer_dir = entry.path();
        if !issuer_dir.is_dir() {
            continue;
        }
        let cert_path = issuer_dir.join(hostname).join(format!("{hostname}.crt"));
        if cert_path.try_exists()? {
            let issuer_kind = entry.file_name().to_string_lossy().into_owned();
            return Ok(Some((issuer_kind, cert_path)));
        }
    }
    Ok(None)
}

/// Parsed view of a Caddy-managed certificate read off the certmagic
/// data directory. Used by the OI rollup to show issuer / expiry / "last
/// issued at" for hostnames whose strategy is the runtime default
/// (Caddy's automatic TLS).
#[derive(Debug)]
pub struct CaddyCertView {
    /// The issuer subdir name on disk: `local` for Caddy's internal CA,
    /// or the host of the ACME directory URL (for example
    /// `acme-v02.api.letsencrypt.org-directory`).
    pub issuer_kind: String,
    /// Issuer DN as encoded in the leaf certificate.
    pub issuer: Option<String>,
    pub not_before: Option<i64>,
    pub not_after: Option<i64>,
    pub self_signed: bool,
}

/// What the proxy's cache says about a hostname.
#[derive(Debug)]
pub enum CertRead {
    /// The cache was read and holds no certificate for the hostname.
    Absent,
    /// A certificate file is there, or the cache could not be read, and what
    /// it holds could not be determined.
    Unreadable,
    Present(CaddyCertView),
}

/// Read and parse the Caddy-managed cert for `hostname` from disk.
pub fn read_cert_state(caddy_data_path: &Path, hostname: &str) -> CertRead {
    let (issuer_kind, cert_path) = match cert_path_for(caddy_data_path, hostname) {
        Ok(Some(found)) => found,
        Ok(None) => return CertRead::Absent,
        Err(_) => return CertRead::Unreadable,
    };
    let Ok(pem) = std::fs::read_to_string(&cert_path) else {
        return CertRead::Unreadable;
    };
    let Ok(parsed) = crate::runtime::tls::parse::parse_chain(&pem) else {
        return CertRead::Unreadable;
    };
    CertRead::Present(CaddyCertView {
        issuer_kind,
        issuer: parsed.metadata.issuer,
        not_before: parsed.metadata.not_before,
        not_after: parsed.metadata.not_after,
        self_signed: parsed.metadata.self_signed,
    })
}

/// For each `(instance, hostname)` pair, check the cert cache and return any
/// `cert_valid` observation that should be persisted. Hostnames whose cert is
/// not yet present produce no observation here; the fault layer is responsible
/// for filing `cert_acquisition_failed` after a deadline.
pub(crate) fn observe(
    caddy_data_path: &Path,
    targets: &[(ResourceInstance, String)],
) -> Vec<(ResourceInstance, &'static str, Value)> {
    let mut out = Vec::new();
    for (instance, hostname) in targets {
        if cert_present(caddy_data_path, hostname) {
            out.push((
                instance.clone(),
                "cert_valid",
                json!({ "hostname": hostname }),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::resource::ResourceKind;

    fn ingress_instance(name: &str) -> ResourceInstance {
        ResourceInstance::new_singleton(
            seedling_protocol::names::AppName::new("test-app").unwrap(),
            ResourceKind::Ingress,
            name,
        )
    }

    #[test]
    fn cert_present_false_when_dir_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(!cert_present(tmp.path(), "example.com"));
    }

    #[test]
    fn cert_present_true_when_file_in_local_issuer() {
        let tmp = tempfile::TempDir::new().unwrap();
        let issuer_dir = tmp.path().join("caddy/certificates/local/example.com");
        std::fs::create_dir_all(&issuer_dir).unwrap();
        std::fs::write(issuer_dir.join("example.com.crt"), b"PEM").unwrap();
        assert!(cert_present(tmp.path(), "example.com"));
    }

    #[test]
    fn cert_present_true_for_acme_issuer() {
        let tmp = tempfile::TempDir::new().unwrap();
        let issuer = "acme-v02.api.letsencrypt.org-directory";
        let issuer_dir = tmp
            .path()
            .join(format!("caddy/certificates/{issuer}/real.example.com"));
        std::fs::create_dir_all(&issuer_dir).unwrap();
        std::fs::write(issuer_dir.join("real.example.com.crt"), b"PEM").unwrap();
        assert!(cert_present(tmp.path(), "real.example.com"));
    }

    #[test]
    fn observe_emits_only_for_present_certs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let issuer_dir = tmp
            .path()
            .join("caddy/certificates/local/has-cert.example.com");
        std::fs::create_dir_all(&issuer_dir).unwrap();
        std::fs::write(issuer_dir.join("has-cert.example.com.crt"), b"PEM").unwrap();

        let targets = vec![
            (
                ingress_instance("public"),
                "has-cert.example.com".to_string(),
            ),
            (ingress_instance("other"), "missing.example.com".to_string()),
        ];
        let obs = observe(tmp.path(), &targets);
        assert_eq!(obs.len(), 1, "only the present cert should produce an obs");
        assert_eq!(obs[0].1, "cert_valid");
        assert_eq!(obs[0].2["hostname"], "has-cert.example.com");
    }
}
