//! The sessions table: the mount translations of live shell sessions.
//!
//! A shell session's container sits on a pod network of its own and needs its
//! mounts translated from the moment it starts, without waiting for the
//! translation table's next apply. Its translations are held here, apart from
//! the translation table, and the table is rewritten whole from every
//! session's translations in one batch on each change.

use std::collections::BTreeMap;

use nftables::{
    batch::Batch,
    schema::{FlushObject, NfCmd, NfListObject},
    types::{NfChainType, NfHook},
};

use seedling_core::system::types::MountRule;

use super::nft::{PRIO_DSTNAT, base_chain, mount_rule_stmts, rule_obj, table};

pub(super) const TABLE: &str = "seedling_sessions";
pub(super) const CHAIN_PRE: &str = "prerouting";

/// The batch that replaces the sessions table with the translations each
/// session in `sessions` holds.
pub(super) fn sessions_batch(sessions: &BTreeMap<String, Vec<MountRule>>) -> Batch<'static> {
    let mut batch = Batch::new();
    batch.add(NfListObject::Table(table(TABLE)));
    batch.add_cmd(NfCmd::Flush(FlushObject::Table(table(TABLE))));
    batch.add(prerouting_chain());
    // spec: INFRA#mount-endpoint-addressing
    for rule in sessions.values().flatten() {
        for stmts in mount_rule_stmts(rule) {
            batch.add(rule_obj(TABLE, CHAIN_PRE, stmts));
        }
    }
    batch
}

/// The batch that deletes the sessions table, whatever it holds, and creates
/// it again empty.
pub(super) fn reset_batch() -> Batch<'static> {
    let mut batch = Batch::new();
    batch.add(NfListObject::Table(table(TABLE)));
    batch.add_cmd(NfCmd::Delete(NfListObject::Table(table(TABLE))));
    batch.add(NfListObject::Table(table(TABLE)));
    batch.add(prerouting_chain());
    batch
}

/// At the translation table's destination NAT priority, as the mount
/// translations of pods are.
fn prerouting_chain() -> NfListObject<'static> {
    base_chain(
        TABLE,
        CHAIN_PRE,
        NfChainType::NAT,
        NfHook::Prerouting,
        PRIO_DSTNAT,
    )
}
