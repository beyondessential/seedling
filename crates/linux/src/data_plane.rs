use ipnet::Ipv6Net;
use nftables::{
    batch::Batch,
    helper,
    schema::{FlushObject, NfCmd, NfListObject},
};
use rtnetlink::Handle;
use snafu::{ResultExt, Snafu};
use tracing::error;

use seedling_core::system::{
    BoxError, BoxFuture, DataPlane,
    types::{DataPlaneRules, ServiceRoute},
};

mod host_addresses;
mod isolation;
mod nft;
pub mod routes;

#[derive(Debug, Snafu)]
pub enum DataPlaneError {
    #[snafu(display("nftables error: {source}"))]
    Nftables {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("{source}"))]
    HostAddresses { source: host_addresses::ReadError },
    #[snafu(display("rtnetlink error: {source}"))]
    Netlink {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        backtrace: snafu::Backtrace,
    },
}

pub struct NftablesDataPlane {
    node_prefix: Ipv6Net,
    handle: Handle,
}

impl NftablesDataPlane {
    pub fn new(node_prefix: Ipv6Net) -> std::io::Result<Self> {
        let (connection, handle, _) = rtnetlink::new_connection()?;
        tokio::spawn(connection);
        Ok(Self {
            node_prefix,
            handle,
        })
    }
}

impl NftablesDataPlane {
    async fn apply_rules_impl(&self, rules: &DataPlaneRules) -> Result<(), DataPlaneError> {
        // spec: INFRA#pod-traffic-to-the-host
        // The set is rewritten whole with the table, so addresses that cannot
        // be read fail the apply rather than leave the set short of them.
        let held = host_addresses::read(&self.handle)
            .await
            .context(HostAddressesSnafu)?;
        apply_batch(translation_batch(rules, &held.elements())).await
    }

    async fn ensure_isolation_impl(&self) -> Result<(), DataPlaneError> {
        apply_batch(isolation::isolation_batch(&self.node_prefix)).await
    }

    async fn apply_routes_impl(&self, routes: &[ServiceRoute]) -> Result<(), DataPlaneError> {
        self.delete_managed_routes(&self.handle)
            .await
            .map_err(|e| {
                error!(error = %e, "data_plane: delete_managed_routes failed");
                e
            })?;
        for svc in routes {
            self.add_service_route(&self.handle, svc)
                .await
                .map_err(|e| {
                    error!(
                        error = %e,
                        service_ip = %svc.service_ip,
                        backends = svc.backends.len(),
                        "data_plane: add_service_route failed"
                    );
                    e
                })?;
        }
        Ok(())
    }

    async fn clear_all_impl(&self) -> Result<(), DataPlaneError> {
        let mut batch = Batch::new();
        batch.add_cmd(NfCmd::Delete(NfListObject::Table(nft::table(nft::TABLE))));
        let ruleset = batch.to_nftables();
        let _ = helper::apply_ruleset_async(&ruleset).await;
        self.delete_managed_routes(&self.handle).await
    }
}

impl DataPlane for NftablesDataPlane {
    fn apply_rules<'a>(&'a self, rules: &'a DataPlaneRules) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.apply_rules_impl(rules).await.map_err(Into::into) })
    }

    fn apply_routes<'a>(
        &'a self,
        routes: &'a [ServiceRoute],
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.apply_routes_impl(routes).await.map_err(Into::into) })
    }

    fn ensure_isolation<'a>(&'a self) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.ensure_isolation_impl().await.map_err(Into::into) })
    }

    fn clear_all<'a>(&'a self) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.clear_all_impl().await.map_err(Into::into) })
    }
}

/// The batch that replaces the translation table with `rules`, its
/// `host_v4_nat64` set holding `host_v4_nat64`.
// spec: INFRA#host-originated-traffic
fn translation_batch(rules: &DataPlaneRules, host_v4_nat64: &[Ipv6Net]) -> Batch<'static> {
    let mut batch = Batch::new();
    batch.add(NfListObject::Table(nft::table(nft::TABLE)));
    batch.add_cmd(NfCmd::Flush(FlushObject::Table(nft::table(nft::TABLE))));
    batch.add(nft::prerouting_chain());
    batch.add(nft::output_chain());
    batch.add(nft::postrouting_chain());
    for cmd in nft::replace_host_v4_nat64(host_v4_nat64) {
        batch.add_cmd(cmd);
    }

    for rule in &rules.ingress {
        for stmts in nft::ingress_rule_stmts(rule) {
            batch.add(nft::rule_obj(nft::TABLE, nft::CHAIN_PRE, stmts));
        }
        for stmts in nft::nat64_ingress_rule_stmts(rule) {
            batch.add(nft::rule_obj(nft::TABLE, nft::CHAIN_PRE, stmts));
        }
        for stmts in nft::output_ingress_rule_stmts(rule) {
            batch.add(nft::rule_obj(nft::TABLE, nft::CHAIN_OUT, stmts));
        }
    }

    for rule in &rules.mounts {
        for stmts in nft::mount_rule_stmts(rule) {
            batch.add(nft::rule_obj(nft::TABLE, nft::CHAIN_PRE, stmts));
        }
    }

    for rule in &rules.service_dnat {
        for stmts in nft::service_dnat_prerouting_stmts(rule) {
            batch.add(nft::rule_obj(nft::TABLE, nft::CHAIN_PRE, stmts));
        }
        for stmts in nft::service_dnat_output_stmts(rule) {
            batch.add(nft::rule_obj(nft::TABLE, nft::CHAIN_OUT, stmts));
        }
    }

    for stmts in nft::loopback_masquerade_stmts() {
        batch.add(nft::rule_obj(nft::TABLE, nft::CHAIN_POST, stmts));
    }
    batch
}

async fn apply_batch(batch: Batch<'_>) -> Result<(), DataPlaneError> {
    helper::apply_ruleset_async(&batch.to_nftables())
        .await
        .map_err(|e| DataPlaneError::Nftables {
            source: Box::new(e),
            backtrace: std::backtrace::Backtrace::capture(),
        })
}

#[cfg(test)]
mod tests;
