mod handler;
mod registry;
mod session;
#[cfg(test)]
mod tests;
mod volume_session;

pub(crate) use handler::{list_shells, resize_shell, stop_shell};
#[cfg(test)]
pub(crate) use registry::StopSignal;
pub use registry::{ShellRecord, ShellRegistry, ShellSession};
pub(crate) use session::open_shell_session;
pub(crate) use volume_session::open_volume_shell_session;

/// The /64 for a session's own pod network, allocated and recorded.
// spec: INFRA#pod-networks
pub(super) fn allocate_session_prefix(
    state: &crate::oi::state::OiState,
    net_name: &str,
    kind: crate::defs::resource::ResourceKind,
) -> Result<ipnet::Ipv6Net, String> {
    let node_prefix = state.node_prefix;
    let name = net_name.to_owned();
    state
        .db
        .call(move |db| {
            crate::runtime::pod_networks::allocated_prefix(db, &node_prefix, &name, kind)
        })
        .map_err(|e| e.to_string())
}

/// Remove a session's pod network and its mount translations, and free its
/// prefix.
// spec: INFRA#pod-networks
// spec: INFRA#mount-endpoint-addressing
pub(super) async fn remove_session_network(state: &crate::oi::state::OiState, net_name: &str) {
    state
        .driver
        .session_mounts
        .remove_session_network(
            &*state.container_runtime,
            &*state.driver.data_plane,
            &state.db,
            net_name,
        )
        .await;
}
pub use volume_session::{VOLUME_SHELL_IMAGE, build_volume_shell_image};
