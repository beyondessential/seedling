use snafu::Snafu;

use crate::system::{
    DataPlane,
    types::{DataPlaneRules, ServiceRoute},
};

// ---------------------------------------------------------------------------
// Internal error type
// ---------------------------------------------------------------------------

#[derive(Debug, Snafu)]
pub(crate) enum DataPlaneError {
    #[snafu(display("nftables error: {message}"))]
    Nftables { message: String },
    #[snafu(display("rtnetlink error: {message}"))]
    Netlink { message: String },
    #[snafu(display("I/O error: {source}"))]
    Io { source: std::io::Error },
}

// ---------------------------------------------------------------------------
// NftablesDataPlane
// ---------------------------------------------------------------------------

/// `DataPlane` implementation using the `nftables` crate for kernel-level
/// networking rules and `rtnetlink` for IPv6 routing table management.
///
/// All rules live in a single table: `table inet seedling_net {}`.
///
/// **`prerouting` chain** (type nat, hook prerouting, priority dstnat):
/// - `IngressRule`s: DNAT external traffic on ingress ports to Caddy's IPv6 addr.
/// - `MountRule`s: DNAT6 per-pod localmount:port → service-ip:canonical-port.
///
/// **`forward` chain** (type filter, hook forward, priority filter):
/// - Single rule: allow all traffic within the seedling ULA prefix
///   (`fd5e:ed::/24`), covering pod-to-service and Caddy-to-service routing.
///
/// `apply_rules` flushes the table and rewrites all chains in one atomic `nft`
/// transaction. `apply_routes` manages IPv6 host routes via rtnetlink, with
/// ECMP multipath for services with multiple backing instances.
///
/// All methods are currently stubs; implement `apply_rules` (nftables) and
/// `apply_routes` (rtnetlink) in isolation before wiring into the actuator.
pub(crate) struct NftablesDataPlane {
    // TODO: add the following fields once `nftables` and `rtnetlink` crate
    // dependencies are added:
    //
    //   nft: nftables::Nftables,   (or however the crate exposes its handle)
    //   rtnetlink handle obtained from rtnetlink::new_connection()
    _private: (),
}

impl NftablesDataPlane {
    /// Create a new `NftablesDataPlane`.
    ///
    /// At construction time this will eventually:
    /// 1. Open an rtnetlink connection.
    /// 2. Ensure the `seedling_net` nftables table exists (creating it if absent).
    pub(crate) fn new() -> Self {
        Self { _private: () }
    }
}

impl DataPlane for NftablesDataPlane {
    type Error = DataPlaneError;

    async fn apply_rules(&self, _rules: &DataPlaneRules) -> Result<(), Self::Error> {
        todo!("NftablesDataPlane::apply_rules")
    }

    async fn apply_routes(&self, _routes: &[ServiceRoute]) -> Result<(), Self::Error> {
        todo!("NftablesDataPlane::apply_routes")
    }

    async fn clear_all(&self) -> Result<(), Self::Error> {
        todo!("NftablesDataPlane::clear_all")
    }
}
