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
/// The canonical `ipv4only.arpa` A records (RFC 7050).
const IPV4ONLY_CANONICAL: [std::net::Ipv4Addr; 2] = [
    std::net::Ipv4Addr::new(192, 0, 0, 170),
    std::net::Ipv4Addr::new(192, 0, 0, 171),
];

/// Whether an AAAA returned for `ipv4only.arpa` is a DNS64 synthesis.
///
/// `r[infra.nat64.detection]` counts an AAAA as evidence of NAT64 only when
/// it is *outside* the canonical addresses. A resolver stack that hands back
/// the canonical A records in IPv4-mapped form (`::ffff:192.0.0.170`) has
/// synthesised nothing, but any IPv6 result used to be taken as proof — so
/// such a host decided the network already provided NAT64 and declined to
/// activate its own.
///
/// A genuine synthesis embeds the canonical IPv4 under a NAT64 prefix, so
/// the embedded address matching is expected and is not what disqualifies
/// it; being *only* a v4-mapped canonical address is.
fn is_synthesised_aaaa(addr: std::net::Ipv6Addr) -> bool {
    match addr.to_ipv4_mapped() {
        Some(v4) => !IPV4ONLY_CANONICAL.contains(&v4),
        None => true,
    }
}

/// Returns `true` if the network already provides NAT64 (and seedling should
/// not activate its own).
pub async fn detect_external_nat64() -> bool {
    match tokio::net::lookup_host("ipv4only.arpa:0").await {
        Ok(addrs) => {
            for addr in addrs {
                let std::net::IpAddr::V6(v6) = addr.ip() else {
                    continue;
                };
                if !is_synthesised_aaaa(v6) {
                    tracing::debug!(
                        addr = %v6,
                        "ipv4only.arpa AAAA is a canonical address in v6 form, not a synthesis"
                    );
                    continue;
                }
                tracing::info!(
                    addr = %v6,
                    "detected existing NAT64+DNS64 infrastructure via RFC 7050"
                );
                return true;
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

#[cfg(test)]
mod detection_tests {
    use std::net::Ipv6Addr;

    use super::is_synthesised_aaaa;

    // r[verify infra.nat64.detection]
    // A resolver handing back the canonical A records in IPv4-mapped form has
    // synthesised nothing. Taking any AAAA as proof made such a host decide
    // the network already provided NAT64 and decline to activate its own.
    #[test]
    fn a_v4_mapped_canonical_address_is_not_a_synthesis() {
        assert!(!is_synthesised_aaaa(
            "::ffff:192.0.0.170".parse::<Ipv6Addr>().unwrap()
        ));
        assert!(!is_synthesised_aaaa(
            "::ffff:192.0.0.171".parse::<Ipv6Addr>().unwrap()
        ));
    }

    // r[verify infra.nat64.detection]
    // A real synthesis puts the canonical IPv4 under a NAT64 prefix — the
    // well-known 64:ff9b::/96 here — which is not a v4-mapped address.
    #[test]
    fn an_address_under_a_nat64_prefix_is_a_synthesis() {
        assert!(is_synthesised_aaaa(
            "64:ff9b::c000:00aa".parse::<Ipv6Addr>().unwrap()
        ));
        assert!(is_synthesised_aaaa(
            "2001:db8:64::c000:00aa".parse::<Ipv6Addr>().unwrap()
        ));
    }

    // r[verify infra.nat64.detection]
    #[test]
    fn a_v4_mapped_address_that_is_not_canonical_still_counts() {
        assert!(is_synthesised_aaaa(
            "::ffff:203.0.113.1".parse::<Ipv6Addr>().unwrap()
        ));
    }
}
