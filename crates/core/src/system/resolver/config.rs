use std::net::SocketAddr;

/// Generates a CoreDNS Corefile.
///
/// `upstreams` are the addresses CoreDNS will forward all queries to
/// (whitespace-separated in the resulting `forward . ...` directive).
/// Typically a single entry: the resolver-bridge gateway handled by
/// seedling's in-process forwarder, or the explicit list passed via
/// `--dns-upstreams`.
///
/// When `nat64_active` is true, the dns64 plugin is included to synthesise
/// AAAA records under the well-known prefix `64:ff9b::/96`.
// r[impl infra.resolver.config]
pub(crate) fn generate_corefile(upstreams: &[SocketAddr], nat64_active: bool) -> String {
    let mut config = String::from(".:53 {\n");
    config.push_str("    forward .");
    for up in upstreams {
        config.push(' ');
        config.push_str(&format_forward_target(up));
    }
    config.push('\n');
    config.push_str("    cache 30\n");
    // r[impl infra.nat64.dns64]
    if nat64_active {
        config.push_str("    dns64 {\n");
        config.push_str("        prefix 64:ff9b::/96\n");
        config.push_str("    }\n");
    }
    config.push_str("    health :8080\n");
    config.push_str("    errors\n");
    config.push_str("}\n");
    config
}

/// CoreDNS's `forward` plugin wants a bare IP when the port is the
/// default (53), and `host:port` / `[host]:port` only when a
/// non-default port is supplied. Bracketing a bare IPv6 address (with
/// no port) makes the plugin reject the config at load time with
/// `not an IP address or file`.
fn format_forward_target(addr: &SocketAddr) -> String {
    match (addr.ip(), addr.port()) {
        (ip, 53) => ip.to_string(),
        (ip, port) if ip.is_ipv4() => format!("{ip}:{port}"),
        (ip, port) => format!("[{ip}]:{port}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    fn sample_upstreams() -> Vec<SocketAddr> {
        vec![SocketAddr::new(
            IpAddr::V6("fd5e:a5:bac1:fd00::1".parse().unwrap()),
            53,
        )]
    }

    #[test]
    fn corefile_without_nat64() {
        let cf = generate_corefile(&sample_upstreams(), false);
        // Default-port IPv6 must be bare (no brackets) — CoreDNS rejects
        // `[host]` without a port with "not an IP address or file".
        assert!(cf.contains("forward . fd5e:a5:bac1:fd00::1\n"));
        assert!(!cf.contains("[fd5e"));
        assert!(cf.contains("cache 30"));
        assert!(cf.contains("health :8080"));
        assert!(!cf.contains("dns64"));
    }

    #[test]
    fn corefile_with_nat64() {
        let cf = generate_corefile(&sample_upstreams(), true);
        assert!(cf.contains("dns64"));
        assert!(cf.contains("64:ff9b::/96"));
    }

    #[test]
    fn corefile_multiple_upstreams_space_separated() {
        let ups = vec![
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 53),
        ];
        let cf = generate_corefile(&ups, false);
        assert!(cf.contains("forward . 1.1.1.1 8.8.8.8"));
    }

    #[test]
    fn corefile_non_default_port_emitted() {
        let ups = vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 5353)];
        let cf = generate_corefile(&ups, false);
        assert!(cf.contains("forward . 1.1.1.1:5353"));
    }

    #[test]
    fn corefile_ipv6_non_default_port_bracketed() {
        let ups = vec![SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 5353)];
        let cf = generate_corefile(&ups, false);
        assert!(cf.contains("forward . [::1]:5353"));
    }
}
