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
