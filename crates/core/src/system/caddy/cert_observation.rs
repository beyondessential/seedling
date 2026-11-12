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

/// What one look at the proxy's certificate cache found.
pub(crate) struct Observed {
    /// The `cert_valid` observations to persist.
    pub observations: Vec<(ResourceInstance, &'static str, Value)>,
    /// Hostnames whose certificate could not be read, so that whether a
    /// usable certificate is held for them is unknown rather than no.
    pub undetermined: std::collections::BTreeSet<String>,
}

/// For each `(instance, hostname)` pair, check the cert cache and return any
/// `cert_valid` observation that should be persisted. A certificate is valid
/// only inside its validity window, so a file that is there but expired, or
/// not yet valid, produces none. Hostnames whose cert is not yet present
/// produce no observation here; the fault layer is responsible for filing
/// `cert_acquisition_failed` after a deadline.
// spec: OBS#ingresses
pub(crate) fn observe(
    caddy_data_path: &Path,
    targets: &[(ResourceInstance, String)],
    now: jiff::Timestamp,
) -> Observed {
    let mut observed = Observed {
        observations: Vec::new(),
        undetermined: std::collections::BTreeSet::new(),
    };
    for (instance, hostname) in targets {
        match read_cert_state(caddy_data_path, hostname) {
            CertRead::Absent => {}
            CertRead::Unreadable => {
                observed.undetermined.insert(hostname.clone());
            }
            CertRead::Present(view) => match (view.not_before, view.not_after) {
                (Some(from), Some(to)) => {
                    if (from..=to).contains(&now.as_second()) {
                        observed.observations.push((
                            instance.clone(),
                            "cert_valid",
                            json!({ "hostname": hostname }),
                        ));
                    }
                }
                _ => {
                    observed.undetermined.insert(hostname.clone());
                }
            },
        }
    }
    observed
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
    fn a_missing_cache_holds_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(matches!(
            read_cert_state(tmp.path(), "example.com"),
            CertRead::Absent
        ));
    }

    #[test]
    fn a_certificate_under_any_issuer_is_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        for issuer in ["local", "acme-v02.api.letsencrypt.org-directory"] {
            let host = format!("{}.example.com", issuer.len());
            write_cert(
                tmp.path(),
                issuer,
                &host,
                &crate::runtime::tls::test_support::self_signed_pem(&[host.as_str()]),
            );
            assert!(matches!(
                read_cert_state(tmp.path(), &host),
                CertRead::Present(_)
            ));
        }
    }

    fn write_cert(root: &Path, issuer: &str, host: &str, pem: &str) {
        let dir = root.join(format!("caddy/certificates/{issuer}/{host}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{host}.crt")), pem).unwrap();
    }

    // spec: OBS#ingresses
    #[test]
    fn observe_emits_only_for_present_valid_certs() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_cert(
            tmp.path(),
            "local",
            "has-cert.example.com",
            &crate::runtime::tls::test_support::self_signed_pem(&["has-cert.example.com"]),
        );

        let targets = vec![
            (
                ingress_instance("public"),
                "has-cert.example.com".to_string(),
            ),
            (ingress_instance("other"), "missing.example.com".to_string()),
        ];
        let seen = observe(tmp.path(), &targets, jiff::Timestamp::now());
        assert_eq!(
            seen.observations.len(),
            1,
            "only the present cert should produce an obs"
        );
        assert_eq!(seen.observations[0].1, "cert_valid");
        assert_eq!(seen.observations[0].2["hostname"], "has-cert.example.com");
        assert!(seen.undetermined.is_empty());
    }

    // spec: OBS#ingresses
    #[test]
    fn a_certificate_outside_its_validity_window_is_not_observed_valid() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_cert(
            tmp.path(),
            "local",
            "old.example.com",
            &crate::runtime::tls::test_support::expired_pem(&["old.example.com"]),
        );
        let targets = vec![(ingress_instance("public"), "old.example.com".to_string())];

        let seen = observe(tmp.path(), &targets, jiff::Timestamp::now());
        assert!(seen.observations.is_empty(), "an expired file is not valid");
    }

    // spec: OBS#ingresses
    #[test]
    fn a_certificate_that_cannot_be_read_is_undetermined_not_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_cert(tmp.path(), "local", "bad.example.com", "not a certificate");
        let targets = vec![(ingress_instance("public"), "bad.example.com".to_string())];

        let seen = observe(tmp.path(), &targets, jiff::Timestamp::now());
        assert!(seen.observations.is_empty());
        assert!(seen.undetermined.contains("bad.example.com"));
    }
}
