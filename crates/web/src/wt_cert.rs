use std::time::{Duration, SystemTime};

use wtransport::Identity;

const CERT_VALIDITY_DAYS: u32 = 7;
const ROTATION_LOOKAHEAD: Duration = Duration::from_secs(6 * 86400);

pub struct CertEntry {
    pub identity: Identity,
    pub hash: String,
    pub not_after: SystemTime,
}

impl CertEntry {
    fn generate() -> Self {
        // SANs are never checked — browsers validate by hash via serverCertificateHashes.
        let identity = Identity::self_signed_builder()
            .subject_alt_names(["seedling-web"])
            .from_now_utc()
            .validity_days(CERT_VALIDITY_DAYS)
            .build()
            .expect("hardcoded SAN is valid");

        let hash = cert_hash_hex(&identity);
        let not_after =
            SystemTime::now() + Duration::from_secs(u64::from(CERT_VALIDITY_DAYS) * 86400);
        Self {
            identity,
            hash,
            not_after,
        }
    }
}

fn cert_hash_hex(identity: &Identity) -> String {
    let digest = identity.certificate_chain().as_slice()[0].hash();
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

// w[wt.cert]
// w[wt.cert.rotation]
pub struct CertStore {
    pub current: CertEntry,
    pub next: Option<CertEntry>,
}

impl CertStore {
    pub fn new() -> Self {
        let current = CertEntry::generate();
        tracing::info!(hash = %current.hash, "generated initial WT certificate");
        Self {
            current,
            next: None,
        }
    }

    pub fn cert_hashes(&self) -> Vec<String> {
        let mut hashes = vec![self.current.hash.clone()];
        if let Some(next) = &self.next {
            hashes.push(next.hash.clone());
        }
        hashes
    }

    /// Returns true if the current cert was swapped to next.
    pub fn rotate_if_needed(&mut self) -> bool {
        let now = SystemTime::now();

        if self.current.not_after <= now {
            let new_current = self.next.take().unwrap_or_else(CertEntry::generate);
            tracing::info!(hash = %new_current.hash, "rotating WT certificate (current expired)");
            self.current = new_current;
            self.next = None;
            return true;
        }

        let rotation_threshold = self
            .current
            .not_after
            .checked_sub(ROTATION_LOOKAHEAD)
            .unwrap_or(SystemTime::UNIX_EPOCH);

        if self.next.is_none() && now >= rotation_threshold {
            let next = CertEntry::generate();
            tracing::info!(hash = %next.hash, "pre-generating next WT certificate for rotation");
            self.next = Some(next);
        }

        false
    }

    pub fn current_identity(&self) -> Identity {
        self.current.identity.clone_identity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // w[verify wt.cert]
    #[test]
    fn new_generates_current_cert_with_hash() {
        let store = CertStore::new();
        assert_eq!(store.cert_hashes().len(), 1);
        assert_eq!(
            store.cert_hashes()[0].len(),
            64,
            "cert hash is 32 bytes as hex"
        );
        assert!(store.next.is_none());
    }

    // w[verify wt.cert.rotation]
    #[test]
    fn rotate_if_needed_is_noop_for_fresh_cert() {
        let mut store = CertStore::new();
        let before = store.current.hash.clone();
        assert!(!store.rotate_if_needed(), "fresh cert should not rotate");
        assert_eq!(store.current.hash, before);
        assert!(store.next.is_none(), "not yet in rotation window");
    }

    // w[verify wt.cert.rotation]
    #[test]
    fn rotate_if_needed_precomputes_next_when_in_rotation_window() {
        let mut store = CertStore::new();
        // Force the current cert into the rotation window by setting its
        // not_after to just inside ROTATION_LOOKAHEAD from now.
        store.current.not_after = SystemTime::now() + Duration::from_secs(60);
        assert!(
            !store.rotate_if_needed(),
            "current still valid; should not swap yet",
        );
        assert!(
            store.next.is_some(),
            "should have pre-generated the next cert",
        );
        let next_hash = store.next.as_ref().unwrap().hash.clone();
        assert_ne!(next_hash, store.current.hash, "next cert must be distinct");

        // cert_hashes reflects both so clients in rotation window accept either.
        let hashes = store.cert_hashes();
        assert_eq!(hashes.len(), 2);
        assert!(hashes.contains(&store.current.hash));
        assert!(hashes.contains(&next_hash));
    }

    // w[verify wt.cert.rotation]
    #[test]
    fn rotate_if_needed_swaps_expired_current_to_pregenerated_next() {
        let mut store = CertStore::new();
        // Pre-populate a next cert by forcing the rotation window.
        store.current.not_after = SystemTime::now() + Duration::from_secs(60);
        let _ = store.rotate_if_needed();
        let next_hash = store.next.as_ref().unwrap().hash.clone();

        // Now expire the current cert.
        store.current.not_after = SystemTime::now() - Duration::from_secs(1);
        assert!(
            store.rotate_if_needed(),
            "expired current should have rotated",
        );
        assert_eq!(store.current.hash, next_hash, "next became current");
        assert!(store.next.is_none(), "next slot cleared after promotion");
    }

    // w[verify wt.cert.rotation]
    #[test]
    fn rotate_if_needed_generates_new_current_if_next_missing() {
        // If expiry was reached without a pre-generated `next` (e.g. system
        // clock jumped, or the pre-gen was skipped), rotation must still
        // succeed by generating a fresh cert.
        let mut store = CertStore::new();
        let old_hash = store.current.hash.clone();
        store.current.not_after = SystemTime::now() - Duration::from_secs(1);
        assert!(store.rotate_if_needed());
        assert_ne!(store.current.hash, old_hash);
    }
}
