use ipnet::Ipv6Net;
use nftables::{
    batch::Batch,
    helper,
    schema::{FlushObject, NfCmd, NfListObject},
};
use rtnetlink::Handle;
use snafu::Snafu;
use tracing::error;

use crate::system::{
    BoxError, BoxFuture, DataPlane,
    types::{DataPlaneRules, ServiceRoute},
};

mod nft;
mod routes;

#[derive(Debug, Snafu)]
pub(crate) enum DataPlaneError {
    #[snafu(display("nftables error: {source}"))]
    Nftables {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("rtnetlink error: {source}"))]
    Netlink {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
        backtrace: snafu::Backtrace,
    },
}

pub(crate) struct NftablesDataPlane {
    node_prefix: Ipv6Net,
    handle: Handle,
}

impl NftablesDataPlane {
    pub(crate) fn new(node_prefix: Ipv6Net) -> std::io::Result<Self> {
        let (connection, handle, _) = rtnetlink::new_connection()?;
        tokio::spawn(connection);
        Ok(Self {
            node_prefix,
            handle,
        })
    }
}

impl NftablesDataPlane {
    // r[impl infra.dataplane.output-nat]
    async fn apply_rules_impl(&self, rules: &DataPlaneRules) -> Result<(), DataPlaneError> {
        let mut batch = Batch::new();
        batch.add(nft::nft_table());
        batch.add_cmd(NfCmd::Flush(FlushObject::Table(nft::table())));
        batch.add(nft::prerouting_chain());
        batch.add(nft::output_chain());
        batch.add(nft::postrouting_chain());
        batch.add(nft::forward_chain());

        for rule in &rules.ingress {
            for stmts in nft::ingress_rule_stmts(rule) {
                batch.add(nft::rule_obj(nft::CHAIN_PRE, stmts));
            }
            for stmts in nft::output_ingress_rule_stmts(rule) {
                batch.add(nft::rule_obj(nft::CHAIN_OUT, stmts));
            }
        }

        for rule in &rules.mounts {
            for stmts in nft::mount_rule_stmts(rule) {
                batch.add(nft::rule_obj(nft::CHAIN_PRE, stmts));
            }
        }

        for rule in &rules.service_dnat {
            for stmts in nft::service_dnat_rule_stmts(rule) {
                batch.add(nft::rule_obj(nft::CHAIN_PRE, stmts.clone()));
                batch.add(nft::rule_obj(nft::CHAIN_OUT, stmts));
            }
        }

        for stmts in nft::loopback_masquerade_stmts() {
            batch.add(nft::rule_obj(nft::CHAIN_POST, stmts));
        }

        // r[impl infra.dataplane.forward-policy]
        batch.add(nft::rule_obj(
            nft::CHAIN_FWD,
            nft::ct_state_established_related_accept(),
        ));
        batch.add(nft::rule_obj(nft::CHAIN_FWD, nft::ct_status_dnat_accept()));
        batch.add(nft::rule_obj(
            nft::CHAIN_FWD,
            nft::seedling_forward_stmts(&self.node_prefix),
        ));
        batch.add(nft::rule_obj(
            nft::CHAIN_FWD,
            nft::drop_unsolicited_inbound_stmts(&self.node_prefix),
        ));

        let ruleset = batch.to_nftables();
        helper::apply_ruleset_async(&ruleset)
            .await
            .map_err(|e| DataPlaneError::Nftables {
                source: Box::new(e),
                backtrace: std::backtrace::Backtrace::capture(),
            })
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
        batch.add_cmd(NfCmd::Delete(NfListObject::Table(nft::table())));
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

    fn clear_all<'a>(&'a self) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.clear_all_impl().await.map_err(Into::into) })
    }
}

#[cfg(test)]
mod tests;
