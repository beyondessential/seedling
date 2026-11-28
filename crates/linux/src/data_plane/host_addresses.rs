//! The host's IPv4 addresses as a pod reaches them through NAT64: in
//! `64:ff9b::` form, kept in the translation table's `host_v4_nat64` set so
//! that pod traffic to an ingress port on one of them is redirected to the
//! proxy before NAT64 translates it.
//!
//! Each apply of the translation table writes the set whole from the
//! addresses the host holds at that moment. Between applies a listener
//! follows the kernel's address notifications and adds or deletes single
//! elements. Both write under one lock, so a notification cannot land in a
//! table that is about to be replaced.
//!
//! The listener treats every failure (subscribing, reading the addresses,
//! writing the set, the subscription closing or overrunning) as transient.
//! It reports the failure for the reconciler to file, then subscribes again
//! after a delay that doubles up to a cap and stays there, and on subscribing
//! rewrites the set from the addresses the host then holds, since it may have
//! missed notifications in between.

use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, Mutex as StdMutex, PoisonError},
    time::Duration,
};

use futures_util::{StreamExt, TryStreamExt};
use ipnet::{Ipv4Net, Ipv6Net};
use nftables::{batch::Batch, schema::NfCmd};
use rtnetlink::{
    Handle, MulticastGroup,
    packet_core::{NetlinkMessage, NetlinkPayload},
    packet_route::{
        AddressFamily, RouteNetlinkMessage,
        address::{AddressAttribute, AddressMessage},
    },
};
use snafu::{ResultExt, Snafu};
use tokio::{sync::Mutex, task::JoinHandle};

use seedling_core::{
    runtime::nat64_prefix::{NAT64_PREFIX_NET, synth_v4},
    system::{proxy_ipv4_subnet, resolver::resolver_ipv4_subnet},
};

use super::{DataPlaneError, apply_batch, nft};

/// The delay before subscribing again after the first failure.
const FIRST_RETRY_DELAY: Duration = Duration::from_secs(1);

/// The longest delay between subscriptions. Subscribing goes on at this
/// spacing for as long as it keeps failing.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

/// The addresses the `host_v4_nat64` set was last written from, or `None`
/// while no translation table is known to be installed, so there is no set to
/// write to. The apply and the listener write the set only while holding it.
pub(super) type WrittenSet = Arc<Mutex<Option<HostAddrs>>>;

/// Why the listener is not following the host's addresses, or `None` while it
/// is.
pub(super) type ListenerFailure = Arc<StdMutex<Option<String>>>;

#[derive(Debug, Snafu)]
pub enum ReadError {
    #[snafu(display("reading the host's IPv4 addresses failed: {source}"))]
    Dump { source: rtnetlink::Error },
    #[snafu(display(
        "the kernel reported an IPv4 address on interface {index} without the address"
    ))]
    Unaddressed { index: u32 },
}

/// The IPv4 addresses the host holds, each with the interface holding it, so
/// that an address held on two interfaces stays until both let it go.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct HostAddrs(BTreeSet<(u32, Ipv4Addr)>);

/// A change the kernel reports to the addresses an interface holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AddrEvent {
    Held { index: u32, addr: Ipv4Addr },
    Released { index: u32, addr: Ipv4Addr },
}

/// A change to the `host_v4_nat64` set's elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ElementChange {
    Add(Ipv6Net),
    Delete(Ipv6Net),
}

impl ElementChange {
    pub(super) fn cmd(self) -> NfCmd<'static> {
        match self {
            Self::Add(element) => nft::add_host_v4_nat64(element),
            Self::Delete(element) => nft::delete_host_v4_nat64(element),
        }
    }
}

impl HostAddrs {
    #[cfg(test)]
    pub(super) fn from_held(held: impl IntoIterator<Item = (u32, Ipv4Addr)>) -> Self {
        Self(held.into_iter().collect())
    }

    /// The `host_v4_nat64` set's elements for these addresses.
    pub(super) fn elements(&self) -> Vec<Ipv6Net> {
        set_elements(self.0.iter().map(|(_, addr)| *addr))
    }

    fn holds(&self, addr: Ipv4Addr) -> bool {
        self.0.iter().any(|(_, held)| *held == addr)
    }

    /// Record `event`, returning the change it makes to the set's elements.
    ///
    /// Recording an event already reflected is no change, so a notification
    /// that raced a read of every address leaves the set as it should be.
    // spec: INFRA#pod-traffic-to-the-host
    pub(super) fn record(&mut self, event: AddrEvent) -> Option<ElementChange> {
        match event {
            AddrEvent::Held { index, addr } => {
                let newly_held = !self.holds(addr);
                self.0.insert((index, addr));
                newly_held
                    .then(|| address_element(addr))
                    .flatten()
                    .map(ElementChange::Add)
            }
            AddrEvent::Released { index, addr } => {
                let released = self.0.remove(&(index, addr)) && !self.holds(addr);
                released
                    .then(|| address_element(addr))
                    .flatten()
                    .map(ElementChange::Delete)
            }
        }
    }
}

/// The IPv4 subnets of the proxy and resolver networks, which are in the set
/// whatever addresses the host holds.
// spec: INFRA#pod-traffic-to-the-host
fn infrastructure_subnets() -> [Ipv4Net; 2] {
    [proxy_ipv4_subnet(), resolver_ipv4_subnet()]
}

/// `net` in the NAT64 prefix.
fn nat64_net(net: Ipv4Net) -> Ipv6Net {
    Ipv6Net::new(
        synth_v4(net.network()),
        NAT64_PREFIX_NET.prefix_len() + net.prefix_len(),
    )
    .expect("an IPv4 prefix within a /96 is a valid IPv6 prefix")
}

/// The single element standing for `addr`, or `None` where an
/// infrastructure subnet already covers it. The set holds intervals, which
/// must not overlap.
fn address_element(addr: Ipv4Addr) -> Option<Ipv6Net> {
    if infrastructure_subnets()
        .iter()
        .any(|net| net.contains(&addr))
    {
        return None;
    }
    Some(nat64_net(Ipv4Net::from(addr)))
}

/// The `host_v4_nat64` set's elements: the proxy and resolver subnets, and
/// each of `addrs` outside them, once, in NAT64 form.
// spec: INFRA#pod-traffic-to-the-host
pub(super) fn set_elements(addrs: impl IntoIterator<Item = Ipv4Addr>) -> Vec<Ipv6Net> {
    let mut elements: BTreeSet<Ipv6Net> = infrastructure_subnets()
        .into_iter()
        .map(nat64_net)
        .collect();
    elements.extend(addrs.into_iter().filter_map(address_element));
    elements.into_iter().collect()
}

/// The IPv4 address `msg` reports and the interface holding it, or `None` for
/// an address of another family.
pub(super) fn held_v4(msg: &AddressMessage) -> Result<Option<(u32, Ipv4Addr)>, ReadError> {
    if msg.header.family != AddressFamily::Inet {
        return Ok(None);
    }
    let index = msg.header.index;
    // On a point-to-point link the address attribute is the peer's, and the
    // local attribute the host's own; elsewhere both are the host's.
    let local = msg.attributes.iter().find_map(|attr| match attr {
        AddressAttribute::Local(IpAddr::V4(addr)) => Some(*addr),
        _ => None,
    });
    let address = msg.attributes.iter().find_map(|attr| match attr {
        AddressAttribute::Address(IpAddr::V4(addr)) => Some(*addr),
        _ => None,
    });
    match local.or(address) {
        Some(addr) => Ok(Some((index, addr))),
        None => UnaddressedSnafu { index }.fail(),
    }
}

/// Every IPv4 address the host holds now.
pub(super) async fn read(handle: &Handle) -> Result<HostAddrs, ReadError> {
    let mut request = handle.address().get();
    request.message_mut().header.family = AddressFamily::Inet;
    let mut dump = request.execute();
    let mut held = BTreeSet::new();
    while let Some(msg) = dump
        .try_next()
        .await
        .map_err(|source| ReadError::Dump { source })?
    {
        held.extend(held_v4(&msg)?);
    }
    Ok(HostAddrs(held))
}

/// What a notification on the address subscription asks of the listener.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Notice {
    Address(AddrEvent),
    /// The kernel dropped notifications, so the set must be rewritten whole.
    Overrun,
}

/// What `msg` asks of the listener, or `None` for a notification about
/// something else.
pub(super) fn notice(
    msg: NetlinkMessage<RouteNetlinkMessage>,
) -> Result<Option<Notice>, ReadError> {
    Ok(match msg.payload {
        NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewAddress(msg)) => {
            held_v4(&msg)?.map(|(index, addr)| Notice::Address(AddrEvent::Held { index, addr }))
        }
        NetlinkPayload::InnerMessage(RouteNetlinkMessage::DelAddress(msg)) => {
            held_v4(&msg)?.map(|(index, addr)| Notice::Address(AddrEvent::Released { index, addr }))
        }
        NetlinkPayload::Overrun(_) => Some(Notice::Overrun),
        _ => None,
    })
}

#[derive(Debug, Snafu)]
enum FollowError {
    #[snafu(display("subscribing to the host's IPv4 address changes failed: {source}"))]
    Subscribe { source: std::io::Error },
    #[snafu(display("{source}"))]
    Read { source: ReadError },
    #[snafu(display("writing the host's IPv4 addresses to the packet filter failed: {source}"))]
    Write { source: DataPlaneError },
    #[snafu(display("the subscription to the host's IPv4 address changes closed"))]
    Closed,
}

/// A task aborted when this is dropped.
pub(super) struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Start following the host's IPv4 addresses into the set, until the returned
/// guard is dropped.
pub(super) fn spawn_listener(written: WrittenSet, failure: ListenerFailure) -> AbortOnDrop {
    AbortOnDrop(tokio::spawn(follow(written, failure)))
}

fn report(failure: &ListenerFailure, value: Option<String>) {
    *failure.lock().unwrap_or_else(PoisonError::into_inner) = value;
}

async fn follow(written: WrittenSet, failure: ListenerFailure) {
    let mut delay = FIRST_RETRY_DELAY;
    loop {
        let e = follow_once(&written, &failure, &mut delay).await;
        tracing::error!(
            error = %e,
            retry_in = ?delay,
            "following the host's IPv4 addresses failed; subscribing again"
        );
        report(&failure, Some(e.to_string()));
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(MAX_RETRY_DELAY);
    }
}

/// Subscribe, rewrite the set, and follow notifications until something
/// fails. Resets `delay` once subscribed.
async fn follow_once(
    written: &WrittenSet,
    failure: &ListenerFailure,
    delay: &mut Duration,
) -> FollowError {
    let (connection, handle, mut messages) =
        match rtnetlink::new_multicast_connection(&[MulticastGroup::Ipv4Ifaddr]) {
            Ok(subscription) => subscription,
            Err(source) => return FollowError::Subscribe { source },
        };
    let _connection = AbortOnDrop(tokio::spawn(connection));

    if let Err(e) = resync(written, &handle).await {
        return e;
    }
    report(failure, None);
    *delay = FIRST_RETRY_DELAY;
    tracing::info!("following the host's IPv4 addresses");

    while let Some((msg, _)) = messages.next().await {
        let followed = match notice(msg) {
            Ok(Some(Notice::Address(event))) => record(written, event).await,
            Ok(Some(Notice::Overrun)) => resync(written, &handle).await,
            Ok(None) => Ok(()),
            Err(source) => Err(FollowError::Read { source }),
        };
        if let Err(e) = followed {
            return e;
        }
    }
    FollowError::Closed
}

/// Rewrite the set from the addresses the host holds now, if there is a set.
async fn resync(written: &WrittenSet, handle: &Handle) -> Result<(), FollowError> {
    let mut written = written.lock().await;
    if written.is_none() {
        // The next apply writes the set whole.
        return Ok(());
    }
    let held = read(handle).await.context(ReadSnafu)?;
    let mut batch = Batch::new();
    for cmd in nft::replace_host_v4_nat64(&held.elements()) {
        batch.add_cmd(cmd);
    }
    apply_batch(batch).await.context(WriteSnafu)?;
    *written = Some(held);
    Ok(())
}

/// Write the change `event` makes to the set, if there is a set.
async fn record(written: &WrittenSet, event: AddrEvent) -> Result<(), FollowError> {
    let mut written = written.lock().await;
    let Some(addrs) = written.as_mut() else {
        // The next apply writes the set whole.
        return Ok(());
    };
    let mut next = addrs.clone();
    if let Some(change) = next.record(event) {
        let mut batch = Batch::new();
        batch.add_cmd(change.cmd());
        apply_batch(batch).await.context(WriteSnafu)?;
    }
    *addrs = next;
    Ok(())
}
