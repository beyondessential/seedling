use std::{
    collections::HashMap,
    net::{IpAddr, Ipv6Addr},
};

use futures_util::StreamExt;
use ipnet::Ipv6Net;
use rtnetlink::{Handle, packet_route::address::AddressAttribute};
use tracing::error;

use crate::{
    defs::resource::Resource,
    runtime::desired::DesiredState,
    system::translate::proxy::{pod_mount_addr, pod_network_prefix},
};

fn pod_network_name(instance: &crate::runtime::identity::ResourceInstance) -> String {
    format!("seedling-{}", instance.display_name)
}

// r[autonomous.network]
/// For each pod instance whose network bridge is in `bridge_names`, verify
/// that `pod_prefix::2` is assigned to the bridge interface.  If it is absent
/// (e.g. after a crash between `create_network` and the rtnetlink assignment),
/// re-add it.  This closes the crash-recovery gap described in the plan.
pub(super) async fn ensure_mount_endpoints(
    handle: &Handle,
    bridge_names: &HashMap<String, String>,
    desired: &DesiredState,
    node_prefix: &Ipv6Net,
) {
    for dr in &desired.resources {
        match &dr.definition {
            Resource::Deployment(_) | Resource::Job(_) => {}
            _ => continue,
        }

        let net_name = pod_network_name(&dr.instance);
        let bridge_name = match bridge_names.get(&net_name) {
            Some(b) => b.as_str(),
            None => continue,
        };

        let pod_prefix = pod_network_prefix(node_prefix, &dr.instance);
        let endpoint = pod_mount_addr(&pod_prefix);

        if let Err(e) = ensure_address(handle, bridge_name, endpoint).await {
            error!(
                instance = %dr.instance.display_name,
                bridge = %bridge_name,
                addr = %endpoint,
                error = %e,
                "bridge: failed to ensure ::2 mount endpoint"
            );
        }
    }
}

async fn ensure_address(
    handle: &Handle,
    bridge_name: &str,
    addr: Ipv6Addr,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let if_index = match get_if_index(handle, bridge_name).await? {
        Some(idx) => idx,
        None => return Ok(()),
    };

    if is_address_assigned(handle, if_index, addr).await? {
        return Ok(());
    }

    handle
        .address()
        .add(if_index, IpAddr::V6(addr), 64)
        .execute()
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

    Ok(())
}

/// Returns the kernel interface index for `name`, or `Ok(None)` if the
/// interface does not exist yet (ENODEV — expected while the container is
/// still starting up and netavark has not yet created the bridge).
async fn get_if_index(
    handle: &Handle,
    name: &str,
) -> Result<Option<u32>, Box<dyn std::error::Error + Send + Sync>> {
    let mut links = handle.link().get().match_name(name.to_owned()).execute();
    match links.next().await {
        None => Ok(None),
        Some(Ok(link)) => Ok(Some(link.header.index)),
        Some(Err(rtnetlink::Error::NetlinkError(ref msg))) if msg.raw_code().abs() == 19 => {
            // ENODEV: the bridge has not been created yet — netavark only
            // brings it up when a container connects to the network.
            Ok(None)
        }
        Some(Err(e)) => Err(Box::new(e)),
    }
}

async fn is_address_assigned(
    handle: &Handle,
    if_index: u32,
    target: Ipv6Addr,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let mut stream = handle.address().get().execute();
    while let Some(msg) = stream.next().await {
        let msg = msg?;
        if msg.header.index != if_index {
            continue;
        }
        for attr in &msg.attributes {
            if let AddressAttribute::Address(IpAddr::V6(a)) = attr
                && *a == target
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
