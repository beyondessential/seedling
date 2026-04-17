// r[impl observe.ingress.certs]
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
    let cert_root = caddy_data_path.join("caddy").join("certificates");
    let issuers = match std::fs::read_dir(&cert_root) {
        Ok(rd) => rd,
        Err(_) => return false,
    };
    for entry in issuers.flatten() {
        let issuer_dir = entry.path();
        if !issuer_dir.is_dir() {
            continue;
        }
        let cert_path = issuer_dir.join(hostname).join(format!("{hostname}.crt"));
        if cert_path.exists() {
            return true;
        }
    }
    false
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
        ResourceInstance::new_singleton("test-app", ResourceKind::Ingress, name)
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
