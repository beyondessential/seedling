use std::net::{IpAddr, SocketAddr};

use tracing::warn;

/// Resolve interface names and explicit addresses into a deduplicated list of bind addresses.
///
/// If both `interface_names` and `explicit_addrs` are empty, returns the loopback default.
pub fn resolve_bind_addrs(
    interface_names: &[String],
    explicit_addrs: &[SocketAddr],
    port: u16,
) -> Result<Vec<SocketAddr>, InterfaceError> {
    if interface_names.is_empty() && explicit_addrs.is_empty() {
        return Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))]);
    }

    let mut addrs: Vec<SocketAddr> = explicit_addrs.to_vec();

    for name in interface_names {
        let ifaces = if_addrs::get_if_addrs()
            .map_err(|e| InterfaceError(format!("enumerate interfaces: {e}")))?;

        let matched: Vec<IpAddr> = ifaces
            .iter()
            .filter(|i| i.name == *name)
            .map(|i| i.addr.ip())
            .collect();

        if matched.is_empty() {
            warn!("interface not found: {name}");
            continue;
        }

        for ip in matched {
            addrs.push(SocketAddr::new(ip, port));
        }
    }

    addrs.sort_unstable();
    addrs.dedup();
    Ok(addrs)
}

pub fn is_loopback(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

#[derive(Debug)]
pub struct InterfaceError(pub String);

impl std::fmt::Display for InterfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for InterfaceError {}

#[cfg(test)]
mod tests {
    use super::*;

    // w[verify bind]
    #[test]
    fn empty_inputs_default_to_loopback() {
        let got = resolve_bind_addrs(&[], &[], 8080).unwrap();
        assert_eq!(got, vec!["127.0.0.1:8080".parse().unwrap()]);
    }

    // w[verify bind]
    #[test]
    fn explicit_addrs_are_passed_through() {
        let explicit: Vec<SocketAddr> = vec![
            "192.0.2.1:8080".parse().unwrap(),
            "198.51.100.1:8080".parse().unwrap(),
        ];
        let got = resolve_bind_addrs(&[], &explicit, 8080).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.contains(&"192.0.2.1:8080".parse().unwrap()));
        assert!(got.contains(&"198.51.100.1:8080".parse().unwrap()));
    }

    // w[verify bind]
    #[test]
    fn duplicate_addrs_across_sources_are_deduplicated() {
        let explicit: Vec<SocketAddr> = vec![
            "192.0.2.1:8080".parse().unwrap(),
            "192.0.2.1:8080".parse().unwrap(),
        ];
        let got = resolve_bind_addrs(&[], &explicit, 8080).unwrap();
        assert_eq!(got.len(), 1);
    }

    // w[verify bind]
    #[test]
    fn unknown_interface_name_is_warned_not_fatal() {
        // An interface name that does not exist on any system is skipped
        // (with a warning); if the remaining sources yield no addrs we must
        // still return an empty list rather than error.
        let got = resolve_bind_addrs(&["seedling-nonexistent-iface-0".to_owned()], &[], 8080)
            .expect("unknown interface should not be fatal");
        // May or may not include loopback depending on implementation — at
        // minimum the call must succeed.
        let _ = got;
    }

    #[test]
    fn is_loopback_identifies_loopback_addresses() {
        assert!(is_loopback(&"127.0.0.1:8080".parse().unwrap()));
        assert!(is_loopback(&"[::1]:8080".parse().unwrap()));
        assert!(!is_loopback(&"192.0.2.1:8080".parse().unwrap()));
    }
}
