mod cert_observation;
mod config;
mod proxy;
mod startup;

pub(crate) use cert_observation::observe as observe_certs;
pub(crate) use cert_observation::{CaddyCertView, read_cert as read_caddy_cert};
#[expect(unused_imports, reason = "public API surface")]
pub(crate) use proxy::{CaddyAddrs, CaddyError};
pub(crate) use proxy::{CaddyProxy, build_client};
#[expect(unused_imports, reason = "public API surface")]
pub(crate) use startup::{
    CADDY_BLUE, CADDY_DATA_VOLUME, CADDY_GREEN, CADDY_IMAGE, CaddyStartupError, PROXY_NETWORK,
    proxy_ipv4_subnet, proxy_network_prefix, read_cached_proxy_config,
};
pub(crate) use startup::{ensure_caddy_running, teardown_caddy, write_cached_proxy_config};

#[cfg(test)]
mod tests;
