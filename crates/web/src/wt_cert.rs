use std::time::{Duration, SystemTime};

use wtransport::Identity;

const CERT_VALIDITY_DAYS: u32 = 7;
const CERT_WORKING_DAYS: u64 = 6;
const ROTATION_LOOKAHEAD: Duration = Duration::from_secs(CERT_WORKING_DAYS * 86400);

/// A generated cert with its precomputed hash and expiry.
pub struct CertEntry {
    pub identity: Identity,
    pub hash: String,
    pub not_after: SystemTime,
}

impl CertEntry {
    fn generate(sans: &[String]) -> Self {
        let identity = Identity::self_signed_builder()
            .subject_alt_names(sans.iter().map(String::as_str))
            .from_now_utc()
            .validity_days(CERT_VALIDITY_DAYS)
            .build()
            .expect("valid SANs");

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
    let chain = identity.certificate_chain();
    let cert = &chain.as_slice()[0];
    let digest = cert.hash();
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

// w[wt.cert]
// w[wt.cert.rotation]
pub struct CertStore {
    pub current: CertEntry,
    pub next: Option<CertEntry>,
    sans: Vec<String>,
}

impl CertStore {
    pub fn new(sans: Vec<String>) -> Self {
        let current = CertEntry::generate(&sans);
        tracing::info!(hash = %current.hash, "generated initial WT certificate");
        Self {
            current,
            next: None,
            sans,
        }
    }

    /// Hashes to include in POST /connect responses (current + next if overlap window).
    pub fn cert_hashes(&self) -> Vec<String> {
        let mut hashes = vec![self.current.hash.clone()];
        if let Some(next) = &self.next {
            hashes.push(next.hash.clone());
        }
        hashes
    }

    /// Check whether rotation actions are needed; returns true if the current cert was swapped.
    ///
    /// - If current is within 24h of expiry and next is None: generate next.
    /// - If current has expired: swap next → current (or generate fresh if next is None).
    pub fn rotate_if_needed(&mut self) -> bool {
        let now = SystemTime::now();
        let current_expired = self.current.not_after <= now;

        if current_expired {
            let new_current = self
                .next
                .take()
                .unwrap_or_else(|| CertEntry::generate(&self.sans));
            tracing::info!(hash = %new_current.hash, "rotating WT certificate (current expired)");
            self.current = new_current;
            self.next = None;
            return true;
        }

        // Generate next cert if we're in the rotation lookahead window.
        let rotation_threshold = self
            .current
            .not_after
            .checked_sub(ROTATION_LOOKAHEAD)
            .unwrap_or(SystemTime::UNIX_EPOCH);

        if self.next.is_none() && now >= rotation_threshold {
            let next = CertEntry::generate(&self.sans);
            tracing::info!(hash = %next.hash, "pre-generating next WT certificate for rotation");
            self.next = Some(next);
        }

        false
    }

    pub fn current_identity(&self) -> Identity {
        self.current.identity.clone_identity()
    }
}
