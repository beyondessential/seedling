mod config;
mod proxy;
mod startup;

pub(crate) use config::build_caddy_config;
pub(crate) use proxy::{CaddyAddrs, CaddyError, CaddyProxy};
pub(crate) use startup::{
    CADDY_BLUE, CADDY_DATA_VOLUME, CADDY_GREEN, CADDY_IMAGE, CaddyStartupError, PROXY_NETWORK,
    ensure_caddy_running, proxy_ipv4_subnet, proxy_network_prefix, read_cached_proxy_json,
    teardown_caddy, write_cached_proxy_json,
};

#[cfg(test)]
mod tests;
