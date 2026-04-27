//! DNS provider abstraction for ACME DNS-01 challenges.
//!
//! Each provider implementation is responsible for:
//!
//! - Locating the hosted zone that owns a given FQDN (for providers that
//!   expose multiple zones; single-zone providers just return the configured
//!   zone).
//! - Publishing a `_acme-challenge.<host>` TXT record with a given value.
//! - Removing that record once the challenge is complete (or has failed).
//!
//! Provider credentials are deserialized from the JSON blob stored in
//! `tls_dns_providers.config_ciphertext`.

use std::future::Future;
use std::pin::Pin;

use secrecy::ExposeSecret;
use snafu::Snafu;

use super::{DnsProviderEntry, DnsProviderKind};

pub mod route53;

/// Boxed future alias used by the DNS provider trait so the trait stays
/// dyn-compatible.
type DnsFuture<'a> = Pin<Box<dyn Future<Output = Result<(), DnsError>> + Send + 'a>>;

/// A DNS provider that can publish and retract TXT records for ACME-DNS
/// challenges. Implementations must be idempotent: `set_txt` over an
/// existing identical value must succeed, and `clear_txt` against a
/// non-existent record must succeed.
pub trait DnsProvider: Send + Sync {
    /// Publish a TXT record at `_acme-challenge.<name>` with the given
    /// value. Returns once the provider's API has accepted the change;
    /// caller is responsible for waiting on propagation.
    fn set_txt<'a>(&'a self, name: &'a str, value: &'a str) -> DnsFuture<'a>;

    /// Remove a TXT record previously set via [`set_txt`]. Implementations
    /// must tolerate the record already being absent.
    fn clear_txt<'a>(&'a self, name: &'a str, value: &'a str) -> DnsFuture<'a>;
}

#[derive(Debug, Snafu)]
pub enum DnsError {
    #[snafu(display("invalid provider config: {message}"))]
    InvalidConfig { message: String },

    #[snafu(display("provider API error: {message}"))]
    Api { message: String },

    #[snafu(display("no hosted zone found for name {name}"))]
    NoZone { name: String },
}

/// Construct a [`DnsProvider`] from a stored entry.
pub fn build_provider(entry: &DnsProviderEntry) -> Result<Box<dyn DnsProvider>, DnsError> {
    match entry.kind {
        DnsProviderKind::Route53 => {
            let cfg: route53::Config =
                serde_json::from_str(entry.config.expose_secret()).map_err(|e| {
                    InvalidConfigSnafu {
                        message: format!("route53 config: {e}"),
                    }
                    .build()
                })?;
            Ok(Box::new(route53::Route53Provider::new(cfg)))
        }
    }
}

/// Returns the FQDN of the TXT record to publish for an ACME DNS-01
/// challenge against `name`. Always `_acme-challenge.<name>`, with a
/// trailing dot stripped from the input if present.
pub fn challenge_record_name(name: &str) -> String {
    let stripped = name.strip_suffix('.').unwrap_or(name);
    format!("_acme-challenge.{stripped}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_record_name_strips_trailing_dot() {
        assert_eq!(
            challenge_record_name("foo.example.com"),
            "_acme-challenge.foo.example.com"
        );
        assert_eq!(
            challenge_record_name("foo.example.com."),
            "_acme-challenge.foo.example.com"
        );
    }
}
