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

pub mod dns;
pub mod keypair;
pub mod parse;
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
