//! Storage and types for operator-managed TLS certificates and policies.
//!
//! Three kinds of records live here:
//!
//! - [`DnsProviderEntry`] — named credentials for a DNS-01 challenge provider.
//! - [`TlsCertificate`] — a single (hostname, attempt) row covering manual
//!   uploads, CSR-derived certs, and certs issued by the daemon's own ACME
//!   client. Private key material is always stored encrypted.
//! - [`TlsPolicy`] — per-hostname strategy override binding a hostname to
//!   either a DNS provider (acme-dns) or a stored cert (manual).
//!
//! ACME account state lives in [`AcmeAccount`] keyed by directory URL +
//! contact email, so that the renewal task can drive issuance without
//! re-bootstrapping the account on every restart.

pub mod acme;
pub mod dns;
pub mod keypair;
pub mod parse;
pub mod renewal;
pub mod serve;
pub mod store;

use secrecy::SecretString;
use serde::{Deserialize, Serialize};

/// Stored DNS provider credentials, keyed by an operator-chosen name. The
/// `config` is provider-specific JSON; for `route53` it is
/// `{"access_key_id": ..., "secret_access_key": ..., "region": ...}`.
// r[impl tls.dns-provider.lifecycle]
#[derive(Debug, Clone)]
pub struct DnsProviderEntry {
    pub name: String,
    pub kind: DnsProviderKind,
    pub config: SecretString,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Listing-friendly view of a DNS provider entry — no credentials.
#[derive(Debug, Clone, Serialize)]
pub struct DnsProviderSummary {
    pub name: String,
    pub kind: DnsProviderKind,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DnsProviderKind {
    Route53,
}

impl DnsProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Route53 => "route53",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "route53" => Some(Self::Route53),
            _ => None,
        }
    }
}

/// A stored certificate. The same row models manual uploads, CSR-derived
/// certs, and ACME-DNS-issued certs. The `origin` discriminates and drives
/// renewal scheduling.
// r[impl tls.strategy.manual]
// r[impl tls.csr.flow]
#[derive(Debug, Clone)]
pub struct TlsCertificate {
    pub id: i64,
    pub hostname: String,
    pub state: TlsCertState,
    pub origin: TlsCertOrigin,
    /// PEM-encoded leaf chain; populated for `Active` and `Superseded`.
    pub cert_pem: Option<String>,
    /// PEM-encoded CSR; populated only while `state = CsrPending`.
    pub csr_pem: Option<String>,
    /// Encrypted PKCS#8 private key. Always present.
    pub key_ciphertext: Vec<u8>,
    pub key_type: KeyType,
    pub issuer: Option<String>,
    pub not_before: Option<i64>,
    pub not_after: Option<i64>,
    pub serial: Option<String>,
    pub self_signed: bool,
    pub note: Option<String>,
    /// For `origin = AcmeDns`: the account that issued.
    pub acme_account_id: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsCertState {
    CsrPending,
    Active,
    Superseded,
    Failed,
}

impl TlsCertState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CsrPending => "csr_pending",
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "csr_pending" => Some(Self::CsrPending),
            "active" => Some(Self::Active),
            "superseded" => Some(Self::Superseded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsCertOrigin {
    Manual,
    Csr,
    AcmeDns,
}

impl TlsCertOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Csr => "csr",
            Self::AcmeDns => "acme_dns",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "manual" => Some(Self::Manual),
            "csr" => Some(Self::Csr),
            "acme_dns" => Some(Self::AcmeDns),
            _ => None,
        }
    }
}

/// Key types accepted today. ECDSA P-256 only; the enum is kept so adding
/// PQC variants later (ML-DSA once Caddy / public CAs catch up) is a single
/// new arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyType {
    EcdsaP256,
}

impl KeyType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EcdsaP256 => "ecdsa_p256",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ecdsa_p256" => Some(Self::EcdsaP256),
            _ => None,
        }
    }
}

/// Per-hostname operator policy. Hostnames absent from `tls_policies` use
/// the runtime default (ACME HTTP-01 via Caddy).
// r[impl tls.strategy.acme-dns]
// r[impl tls.strategy.manual]
#[derive(Debug, Clone)]
pub enum TlsPolicy {
    AcmeDns { dns_provider: String },
    Manual { cert_id: i64 },
}

#[derive(Debug, Clone)]
pub struct TlsPolicyRow {
    pub hostname: String,
    pub policy: TlsPolicy,
    pub updated_at: i64,
}

/// Persisted ACME account. The same `(directory_url, contact_email)` pair
/// must reuse the same row across daemon restarts.
// r[impl tls.acme.account.persist]
#[derive(Debug, Clone)]
pub struct AcmeAccount {
    pub id: i64,
    pub directory_url: String,
    pub contact_email: String,
    pub account_url: String,
    pub account_key_ciphertext: Vec<u8>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Global TLS settings. Currently just the operator contact email used by
/// every ACME account registration; a single value applied across all
/// hostnames keeps the operator interface from prompting for the same
/// information per binding.
// r[impl tls.settings.contact-email]
#[derive(Debug, Clone, Default)]
pub struct TlsSettings {
    pub contact_email: String,
    pub updated_at: i64,
}

/// Returns whether `pattern` matches `hostname`. Patterns are:
///
/// - `*` — catch-all, matches any hostname.
/// - `*.<suffix>` — **shell-glob-style** subdomain wildcard. Matches any
///   hostname ending in `.<suffix>`, including multi-label
///   subdomains. So `*.example.com` matches `foo.example.com` AND
///   `a.b.example.com`, but not `example.com` itself.
///
///   This is *not* the RFC 6125 DNS-wildcard semantic (which would
///   match exactly one extra label). It's chosen so an operator can
///   say "use this DNS provider for everything under example.com" with
///   a single rule, rather than re-declaring the same policy at every
///   depth. Most-specific-first resolution
///   ([`pattern_specificity`]) means an operator can still pin a
///   sub-zone to a different policy by adding `*.sub.example.com` or
///   `foo.example.com` alongside.
/// - anything else — exact match (case-insensitive).
///
/// More-specific patterns are tried first by [`store::resolve_policy`].
pub fn pattern_matches(pattern: &str, hostname: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let p = pattern.to_ascii_lowercase();
    let h = hostname.to_ascii_lowercase();
    if let Some(suffix) = p.strip_prefix("*.") {
        // Shell-glob: hostname must end with ".<suffix>" and not equal
        // the suffix itself (the wildcard requires at least one extra
        // leading label, but that label may itself contain dots).
        let needle = format!(".{suffix}");
        return h.ends_with(&needle) && h.len() > needle.len();
    }
    p == h
}

/// Specificity score used to pick the best-matching policy when several
/// patterns match a hostname. Higher is more specific. The exact match
/// always beats wildcards; a wildcard with a longer suffix beats a
/// shorter one; the catch-all `*` is least specific.
pub fn pattern_specificity(pattern: &str) -> u32 {
    if pattern == "*" {
        return 0;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        // Wildcards score by suffix length, with the bare `*` at zero,
        // and exact matches beating any wildcard.
        return 1 + suffix.len() as u32;
    }
    // Exact match: rank above any wildcard regardless of suffix length.
    1_000_000 + pattern.len() as u32
}

#[cfg(test)]
mod pattern_tests {
    use super::*;

    #[test]
    fn catchall_matches_anything() {
        assert!(pattern_matches("*", "foo.example.com"));
        assert!(pattern_matches("*", "x"));
    }

    #[test]
    fn dotted_wildcard_requires_extra_label() {
        assert!(pattern_matches("*.example.com", "foo.example.com"));
        // Multi-level: shell-glob style covers any depth, NOT RFC 6125's
        // single-label semantic. A change here is a behaviour change for
        // operator policy and should be made deliberately.
        assert!(pattern_matches("*.example.com", "a.b.example.com"));
        assert!(pattern_matches("*.example.com", "x.y.z.example.com"));
        assert!(!pattern_matches("*.example.com", "example.com"));
        assert!(!pattern_matches("*.example.com", "other.com"));
    }

    #[test]
    fn more_specific_wildcard_beats_broader_one() {
        // Two policies overlap at `foo.bar.example.com`; the longer suffix
        // must score higher so the operator's deliberate sub-zone override
        // wins. This is the contract that makes shell-glob multi-level
        // wildcards safe to use as a catch-all.
        let broad = pattern_specificity("*.example.com");
        let narrow = pattern_specificity("*.bar.example.com");
        assert!(narrow > broad);
        assert!(pattern_matches("*.example.com", "foo.bar.example.com"));
        assert!(pattern_matches("*.bar.example.com", "foo.bar.example.com"));
    }

    #[test]
    fn exact_match_is_case_insensitive() {
        assert!(pattern_matches("Foo.Example.com", "foo.example.com"));
        assert!(!pattern_matches("foo.example.com", "bar.example.com"));
    }

    #[test]
    fn specificity_ordering() {
        let exact = pattern_specificity("foo.example.com");
        let wide = pattern_specificity("*.example.com");
        let wider = pattern_specificity("*.com");
        let star = pattern_specificity("*");
        assert!(exact > wide);
        assert!(wide > wider);
        assert!(wider > star);
    }
}
