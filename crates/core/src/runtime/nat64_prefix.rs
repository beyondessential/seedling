use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::LazyLock;

use ipnet::Ipv6Net;

pub const NAT64_PREFIX_STR: &str = "64:ff9b::/96";

pub static NAT64_PREFIX_NET: LazyLock<Ipv6Net> =
    LazyLock::new(|| NAT64_PREFIX_STR.parse().expect("hard-coded NAT64 prefix"));

/// Synthesise the NAT64 IPv6 form of an IPv4 address per RFC 6052 using the
/// IANA well-known prefix `64:ff9b::/96`. The low 32 bits hold the IPv4
/// address; all higher bits are fixed by the prefix.
// r[impl service.site.address]
pub fn synth_v4(v4: Ipv4Addr) -> Ipv6Addr {
    let octets = v4.octets();
    Ipv6Addr::new(
        0x0064,
        0xff9b,
        0,
        0,
        0,
        0,
        u16::from_be_bytes([octets[0], octets[1]]),
        u16::from_be_bytes([octets[2], octets[3]]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesises_well_known_form() {
        let v4: Ipv4Addr = "192.0.2.10".parse().unwrap();
        let synth = synth_v4(v4);
        let expected: Ipv6Addr = "64:ff9b::c000:20a".parse().unwrap();
        assert_eq!(synth, expected);
    }

    #[test]
    fn synthesises_zero_address() {
        let v4: Ipv4Addr = "0.0.0.0".parse().unwrap();
        assert_eq!(synth_v4(v4), "64:ff9b::".parse::<Ipv6Addr>().unwrap());
    }

    #[test]
    fn synthesises_all_ones() {
        let v4: Ipv4Addr = "255.255.255.255".parse().unwrap();
        assert_eq!(
            synth_v4(v4),
            "64:ff9b::ffff:ffff".parse::<Ipv6Addr>().unwrap()
        );
    }

    #[test]
    fn prefix_constant_parses() {
        assert_eq!(NAT64_PREFIX_NET.prefix_len(), 96);
        assert_eq!(
            NAT64_PREFIX_NET.network(),
            "64:ff9b::".parse::<Ipv6Addr>().unwrap()
        );
    }

    #[test]
    fn synthesised_addresses_lie_in_the_prefix() {
        let v4: Ipv4Addr = "10.20.30.40".parse().unwrap();
        let synth = synth_v4(v4);
        assert!(NAT64_PREFIX_NET.contains(&synth));
    }
}
