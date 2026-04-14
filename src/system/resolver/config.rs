/// Generates a CoreDNS Corefile.
///
/// When `nat64_active` is true, the dns64 plugin is included to synthesise
/// AAAA records under the well-known prefix `64:ff9b::/96`.
pub(crate) fn generate_corefile(nat64_active: bool) -> String {
    let mut config = String::from(".:53 {\n");
    config.push_str("    forward . /etc/resolv.conf\n");
    config.push_str("    cache 30\n");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corefile_without_nat64() {
        let cf = generate_corefile(false);
        assert!(cf.contains("forward . /etc/resolv.conf"));
        assert!(cf.contains("cache 30"));
        assert!(cf.contains("health :8080"));
        assert!(!cf.contains("dns64"));
    }

    #[test]
    fn corefile_with_nat64() {
        let cf = generate_corefile(true);
        assert!(cf.contains("dns64"));
        assert!(cf.contains("64:ff9b::/96"));
    }
}
