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
