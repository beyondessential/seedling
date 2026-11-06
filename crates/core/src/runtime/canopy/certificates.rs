//! Certificates from Canopy, driven by the report tick.
//!
//! Canopy answers a first request as pending and delivers the chain when asked
//! again, so the runtime asks on the cadence it already reports on. Each tick:
//!
//! 1. revoked certificates and condemned keys, learnt from the held-certificate
//!    list in the report's answer, are replaced;
//! 2. every hostname whose decision says Canopy is to be asked is asked;
//! 3. the faults for Canopy trouble are brought into line with what is now so.
//!
//! The decision is [`crate::runtime::tls::state::compute_state`]'s alone: this
//! module acts on it and never makes a decision of its own about which strategy
//! is responsible.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use bestool_canopy::schema::Entitlements;

use crate::{
    oi::state::OiState,
    runtime::{
        faults::{self, FaultKey, FaultMeta, FaultScope},
        tls::{
            AttemptTrigger,
            canopy::{self, Ask, CanopyRelay, Engine},
            chain::{self, Identity, Strategy},
            state::{
                self, CanopyStanding, ChainAct, ChainDecision, Decision, ManagedIngress,
                ServingSource, Snapshot,
            },
            store,
        },
    },
};

/// The fault kind for a Canopy certificate that cannot be had.
// r[impl tls.canopy.fault]
pub const FAULT_KIND: &str = "canopy_certificate_failed";

/// Run one tick's worth of certificate work.
pub async fn run(state: &Arc<OiState>, names: &Entitlements) {
    let managed = {
        let registry = state.registry.read();
        state::managed_ingresses(&registry)
    };
    let engine = Engine::new(
        state.db.clone(),
        Arc::clone(&state.cipher),
        CanopyRelay::new(Arc::clone(&state.canopy)),
    );

    replace_condemned(state, &engine, &managed, names).await;
    ask_canopy(state, &engine, &managed).await;
    sync_faults(state, &managed);
}

/// Load a snapshot with the facts the chain depends on.
fn snapshot(state: &Arc<OiState>, managed: &[ManagedIngress]) -> Option<Snapshot> {
    let env = match state.chain_env(managed) {
        Ok(env) => env,
        Err(e) => {
            tracing::error!(error = %e, "canopy certificates: cannot read the facts a chain depends on");
            return None;
        }
    };
    match state.db.call(Snapshot::load) {
        Ok(snapshot) => Some(snapshot.with_env(env)),
        Err(e) => {
            tracing::error!(error = %e, "canopy certificates: cannot read the TLS state");
            None
        }
    }
}

/// What to say to Canopy for a hostname whose decision is `c`, or `None` when
/// no cycle is running to say it for.
fn ask_for(hostname: &str, c: &ChainDecision, forced: bool) -> Option<Ask> {
    let chain::Cycle::Running(running) = &c.cycle else {
        return None;
    };
    let CanopyStanding::Applies { app_type } = &c.canopy else {
        return None;
    };
    let trigger = if forced {
        AttemptTrigger::Manual
    } else if running.identity.is_none() {
        AttemptTrigger::OnDemand
    } else {
        AttemptTrigger::Renewal
    };
    Some(Ask {
        hostname: hostname.to_owned(),
        app_type: app_type.clone(),
        replacing: running.identity,
        step: running.step,
        since: running.since,
        chain_len: c.strategies.len(),
        canopy_responsible: running.responsible == Strategy::Canopy,
        trigger,
    })
}

// r[impl tls.canopy.revoked]
async fn replace_condemned(
    state: &Arc<OiState>,
    engine: &Engine<CanopyRelay>,
    managed: &[ManagedIngress],
    names: &Entitlements,
) {
    let held = names.certificates.clone();
    let condemned = match state.db.call(move |db| canopy::condemned_by(db, &held)) {
        Ok(condemned) => condemned,
        Err(e) => {
            tracing::error!(error = %e, "canopy certificates: cannot compare held certificates");
            return;
        }
    };
    for c in condemned {
        let Some(snap) = snapshot(state, managed) else {
            return;
        };
        let st = state::compute_state(&snap, &c.hostname);
        let ask = match &st.decision {
            Decision::Chain(chain) => ask_for(&c.hostname, chain, false),
            _ => None,
        };
        tracing::warn!(
            hostname = %c.hostname,
            reason = %c.reason,
            "Canopy certificate no longer to be served; replacing its key"
        );
        match ask {
            Some(ask) => {
                if let Err(e) = engine
                    .revoke_and_reorder(ask, c.cert_id, c.reason.clone())
                    .await
                {
                    tracing::error!(hostname = %c.hostname, error = %e, "canopy certificates: replacing a revoked certificate failed");
                }
            }
            // The hostname has left its chain, or no cycle is running to say
            // where it stood: the certificate must still stop being served, and
            // nothing more can be said about what replaces it.
            None => {
                let id = c.cert_id;
                if let Err(e) = state
                    .db
                    .call(move |db| store::revoke_canopy_certificate(db, id))
                {
                    tracing::error!(hostname = %c.hostname, error = %e, "canopy certificates: stopping a revoked certificate failed");
                }
            }
        }
    }
}

async fn ask_canopy(
    state: &Arc<OiState>,
    engine: &Engine<CanopyRelay>,
    managed: &[ManagedIngress],
) {
    let Some(snap) = snapshot(state, managed) else {
        return;
    };
    let hostnames: BTreeSet<&str> = managed.iter().map(|m| m.hostname.as_str()).collect();
    for hostname in hostnames {
        let st = state::compute_state(&snap, hostname);
        let Decision::Chain(c) = &st.decision else {
            continue;
        };
        let due = match &c.act {
            ChainAct::AskCanopy => true,
            ChainAct::Debounced { .. } | ChainAct::Wait => c.collect_canopy,
            // An operator pause, or nothing that can be asked.
            ChainAct::Blocked { .. } | ChainAct::CannotAskCanopy => false,
        };
        if !due {
            continue;
        }

        // An operator retry is consumed here, in the same write that records
        // the cycle it starts, so a restart cannot lose it.
        let forced = if st.force_retry_at.is_some() {
            let replacing = match c.serving {
                Some(ServingSource::Store { .. }) | Some(ServingSource::Proxy) => match &c.cycle {
                    chain::Cycle::Running(r) => r.identity,
                    chain::Cycle::Idle { .. } => Identity::NONE,
                },
                None => Identity::NONE,
            };
            let now = snap.now;
            let host = hostname.to_owned();
            match state
                .db
                .call(move |db| store::start_cycle_for_force_retry(db, &host, replacing, now))
            {
                Ok(queued) => queued,
                Err(e) => {
                    tracing::error!(%hostname, error = %e, "canopy certificates: cannot start the cycle an operator retry asked for");
                    continue;
                }
            }
        } else {
            false
        };

        let Some(ask) = ask_for(hostname, c, forced) else {
            continue;
        };
        match engine.ask(ask).await {
            Ok(outcome) => tracing::debug!(%hostname, ?outcome, "asked Canopy"),
            Err(e) => {
                tracing::error!(%hostname, error = %e, "canopy certificates: asking Canopy failed")
            }
        }
    }
}

/// Bring the Canopy faults into line with the decisions as they now stand.
///
/// One fault per ingress that declares a hostname Canopy cannot provide a
/// certificate for, keyed by the ingress, so it clears when the ingress no
/// longer needs it and never clears another kind of fault.
// r[impl tls.canopy.fault]
// r[impl tls.canopy-only.untyped]
fn sync_faults(state: &Arc<OiState>, managed: &[ManagedIngress]) {
    let Some(snap) = snapshot(state, managed) else {
        return;
    };
    let mut desired: BTreeMap<FaultKey, (FaultMeta, String)> = BTreeMap::new();
    for ingress in managed {
        let st = state::compute_state(&snap, &ingress.hostname);
        let Decision::Chain(c) = &st.decision else {
            continue;
        };
        let Some(description) = trouble(&ingress.hostname, c) else {
            continue;
        };
        desired.insert(
            FaultKey::new(&ingress.app, FAULT_KIND, ingress.ingress_name.clone()),
            (
                FaultMeta {
                    resource_type: Some("ingress".to_owned()),
                    resource_name: Some(ingress.ingress_name.clone()),
                    instance_id: None,
                },
                description,
            ),
        );
    }

    let descriptions: Vec<String> = desired.values().map(|(_, d)| d.clone()).collect();
    match state
        .db
        .call(move |db| faults::sync_faults(db, &FaultScope::Kind(FAULT_KIND.to_owned()), &desired))
    {
        Ok(outcome) if outcome.filed > 0 => {
            for description in descriptions {
                tracing::error!("{description}");
            }
        }
        Ok(_) => {}
        Err(e) => tracing::error!(error = %e, "canopy certificates: cannot update the faults"),
    }
}

/// Why Canopy cannot provide a hostname's certificate, if it cannot and an
/// operator should be told.
///
/// A hostname that holds a usable certificate and is still inside a strategy's
/// window is not in trouble: the chain is still working on it.
fn trouble(hostname: &str, c: &ChainDecision) -> Option<String> {
    if c.act == ChainAct::CannotAskCanopy {
        return Some(match c.canopy {
            CanopyStanding::NotPublicName => format!(
                "{hostname} is bound to the Canopy-only policy but no public CA can issue for it"
            ),
            _ => format!(
                "{hostname} is bound to the Canopy-only policy but no app declaring it has a \
                 Canopy type, so Canopy cannot be asked"
            ),
        });
    }
    if !c.strategies.contains(&Strategy::Canopy) {
        return None;
    }
    let chain::Cycle::Running(running) = &c.cycle else {
        return None;
    };
    if c.serving.is_none()
        && let Some(reason) = &c.last_error
    {
        return Some(format!(
            "Canopy could not provide a certificate for {hostname}: {reason}"
        ));
    }
    // The whole chain has been through and Canopy was not what produced the
    // replacement.
    // A hostname with nothing to replace that skipped Canopy for want of an
    // offer has not been through Canopy at all, and is not in trouble.
    if !running.identity.is_none()
        && c.strategies.len() > 1
        && running.step + 1 == c.strategies.len()
    {
        let reason = c
            .last_error
            .clone()
            .unwrap_or_else(|| "no certificate arrived in time".to_owned());
        return Some(format!(
            "Canopy did not provide a certificate for {hostname} before it fell through to {}: \
             {reason}",
            running.responsible.as_str()
        ));
    }
    None
}
