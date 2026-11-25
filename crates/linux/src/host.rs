//! Host facts on Linux: egress read from the kernel's routing tables, memory
//! from `/proc/meminfo`, and the rest through the portable detections in
//! `seedling-core`.

use seedling_core::{
    sysconst,
    system::{BoxFuture, host_probes::ProbeSource, nat64},
};

use crate::netinfo;

/// The host's machine identifier.
// spec: LINUX#host-facilities
pub fn machine_id() -> std::io::Result<String> {
    std::fs::read_to_string("/etc/machine-id")
}

/// The host's own resolver, routing tables and kernel.
pub struct HostSource;

impl ProbeSource for HostSource {
    fn external_nat64(&self) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(nat64::detect_external_nat64())
    }

    fn ipv4_egress(&self) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(netinfo::detect_ipv4_egress())
    }

    fn ipv6_egress(&self) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(netinfo::detect_ipv6_egress())
    }

    fn timezone(&self) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { sysconst::detect_timezone() })
    }

    fn available_threads(&self) -> BoxFuture<'_, Result<i64, String>> {
        Box::pin(async { sysconst::detect_available_threads() })
    }

    fn available_memory(&self) -> BoxFuture<'_, Result<i64, String>> {
        Box::pin(async { detect_available_memory() })
    }

    fn node_name(&self) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { sysconst::detect_node_name() })
    }
}

/// Memory the script may plan around: 90% of the host's `MemTotal`,
/// reserving 10% for the daemon itself, the runtime, and the kernel's page
/// cache.
// spec: CONST#host-constants
fn detect_available_memory() -> Result<i64, String> {
    let contents = std::fs::read_to_string("/proc/meminfo")
        .map_err(|e| format!("cannot read /proc/meminfo: {e}"))?;
    let total = parse_mem_total_bytes(&contents)?;
    let available = total.saturating_mul(9) / 10;
    if available <= 0 {
        return Err(format!("the host reports {total} bytes of memory"));
    }
    Ok(available)
}

/// `MemTotal` from the contents of `/proc/meminfo`, in bytes.
fn parse_mem_total_bytes(meminfo: &str) -> Result<i64, String> {
    let line = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .ok_or("/proc/meminfo has no MemTotal line")?;
    let mut parts = line.split_whitespace();
    let (Some(num), Some(unit)) = (parts.next(), parts.next()) else {
        return Err(format!("MemTotal {line:?} is not a number and a unit"));
    };
    let value = num
        .parse::<i64>()
        .map_err(|e| format!("MemTotal {num:?} is not a number: {e}"))?;
    let multiplier: i64 = match unit {
        "B" => 1,
        "kB" | "KB" => 1024,
        "MB" => 1024 * 1024,
        "GB" => 1024 * 1024 * 1024,
        other => return Err(format!("MemTotal has an unknown unit {other:?}")),
    };
    Ok(value.saturating_mul(multiplier))
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: CONST#host-constants
    #[test]
    fn mem_total_is_read_in_bytes_and_anything_else_is_undetermined() {
        assert_eq!(
            parse_mem_total_bytes("MemFree: 1 kB\nMemTotal:   2048 kB\n"),
            Ok(2048 * 1024)
        );
        assert!(parse_mem_total_bytes("MemFree: 1 kB\n").is_err());
        assert!(parse_mem_total_bytes("MemTotal: lots kB\n").is_err());
        assert!(parse_mem_total_bytes("MemTotal: 12 parsecs\n").is_err());
    }

    // spec: CONST#host-constants
    #[test]
    fn this_hosts_memory_is_determined_and_positive() {
        assert!(detect_available_memory().unwrap() > 0);
    }
}
