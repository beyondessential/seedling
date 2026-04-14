use std::fmt;

use clap::ValueEnum;

/// Controls whether the runtime provides its own NAT64 translator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum Nat64Mode {
    /// Probe for existing NAT64 infrastructure on startup; enable if absent.
    #[default]
    Auto,
    /// Always provide NAT64.
    Enabled,
    /// Never provide NAT64.
    Disabled,
}

impl fmt::Display for Nat64Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Enabled => f.write_str("enabled"),
            Self::Disabled => f.write_str("disabled"),
        }
    }
}

/// Probes for existing NAT64+DNS64 infrastructure using RFC 7050.
///
/// Returns `true` if the network already provides NAT64 (and seedling should
/// not activate its own).
pub async fn detect_external_nat64() -> bool {
    match tokio::net::lookup_host("ipv4only.arpa:0").await {
        Ok(addrs) => {
            for addr in addrs {
                if addr.is_ipv6() {
                    tracing::info!(
                        addr = %addr.ip(),
                        "detected existing NAT64+DNS64 infrastructure via RFC 7050"
                    );
                    return true;
                }
            }
            tracing::info!("no NAT64+DNS64 detected (ipv4only.arpa returned no AAAA records)");
            false
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "NAT64 detection failed (could not resolve ipv4only.arpa); assuming no NAT64"
            );
            false
        }
    }
}

/// Determines whether seedling should activate its own NAT64 translator.
pub async fn should_activate_nat64(mode: Nat64Mode) -> bool {
    match mode {
        Nat64Mode::Enabled => true,
        Nat64Mode::Disabled => false,
        Nat64Mode::Auto => !detect_external_nat64().await,
    }
}
