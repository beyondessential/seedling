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
/// `NAT64#detection` counts an AAAA as evidence of NAT64 only when
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

/// Whether the network already provides NAT64 and DNS64, which is the
/// decision `ipv4only.arpa` gives once it is answered.
///
/// `Err` is a detection that could not be made: the lookup failed, or came
/// back with no answer at all. That is neither the network providing NAT64
/// nor its lacking it, and is not reported as either.
// spec: NAT64#detection
pub async fn detect_external_nat64() -> Result<bool, String> {
    match tokio::net::lookup_host("ipv4only.arpa:0").await {
        Ok(addrs) => {
            let mut answered = false;
            for addr in addrs {
                answered = true;
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
                return Ok(true);
            }
            if !answered {
                return Err("the lookup of ipv4only.arpa received no answer".to_owned());
            }
            tracing::info!("no NAT64+DNS64 detected (ipv4only.arpa returned no AAAA records)");
            Ok(false)
        }
        Err(e) => Err(format!("the lookup of ipv4only.arpa failed: {e}")),
    }
}

/// Whether the runtime provides its own NAT64 translator, and why that may
/// rest on a detection that could not be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activation {
    pub active: bool,
    /// Why detection was undetermined, when it was. NAT64 is active meanwhile.
    pub undetermined: Option<String>,
}

/// Determines whether seedling should activate its own NAT64 translator.
///
/// Decided once, at start. While detection is undetermined NAT64 is active,
/// and a detection that succeeds later does not change that until the next
/// start.
// spec: NAT64#detection
pub async fn should_activate_nat64(mode: Nat64Mode) -> Activation {
    match mode {
        Nat64Mode::Enabled => Activation {
            active: true,
            undetermined: None,
        },
        Nat64Mode::Disabled => Activation {
            active: false,
            undetermined: None,
        },
        Nat64Mode::Auto => activation_from_detection(detect_external_nat64().await),
    }
}

/// The activation a detection gives.
// spec: NAT64#detection
pub fn activation_from_detection(detection: Result<bool, String>) -> Activation {
    match detection {
        Ok(external) => Activation {
            active: !external,
            undetermined: None,
        },
        Err(why) => Activation {
            active: true,
            undetermined: Some(why),
        },
    }
}

#[cfg(test)]
mod detection_tests {
    use std::net::Ipv6Addr;

    use super::{activation_from_detection, is_synthesised_aaaa};

    // spec: NAT64#detection
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

    // spec: NAT64#detection
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

    // spec: NAT64#detection
    #[test]
    fn a_v4_mapped_address_that_is_not_canonical_still_counts() {
        assert!(is_synthesised_aaaa(
            "::ffff:203.0.113.1".parse::<Ipv6Addr>().unwrap()
        ));
    }

    // spec: NAT64#detection
    #[test]
    fn an_undetermined_detection_activates_nat64_and_says_why() {
        let undetermined = activation_from_detection(Err("no answer".to_owned()));
        assert!(undetermined.active);
        assert_eq!(undetermined.undetermined.as_deref(), Some("no answer"));

        assert!(!activation_from_detection(Ok(true)).active);
        let absent = activation_from_detection(Ok(false));
        assert!(absent.active && absent.undetermined.is_none());
    }
}
