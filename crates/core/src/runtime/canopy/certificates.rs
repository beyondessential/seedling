//! Certificates from Canopy, driven by the report tick.
//!
//! Canopy answers a first request as pending and delivers the chain when asked
//! again, so the runtime asks on the cadence it already reports on. Each tick:
//!
//! 1. revoked certificates and condemned keys, learnt from the held-certificate
//!    list in the report's answer, are replaced;
//! 2. every hostname whose decision says Canopy is to be asked is asked.
//!
//! The faults for Canopy trouble are brought into line with what is now so on
//! every tick, whether or not a report got through: see [`sync_all_faults`].
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
            canopy::{self, Ask, CanopyRelay, CertificateRelay, Engine},
            chain::{self, Identity, Strategy},
            state::AUTO_RETRY_DEBOUNCE_SECS,
            state::{
                self, CanopyStanding, ChainAct, ChainDecision, Decision, ManagedIngress,
                ServingSource, Snapshot,
            },
            store,
        },
    },
};

/// The most hostnames one tick asks Canopy about.
///
/// Each request waits on the relay, which answers within its own timeout of a
/// minute at worst, and they are made one after another so that a burst of them
/// cannot be what makes Canopy slow. A tick that reaches its fill leaves the
/// rest to the next one, which decides again from what is recorded.
const MOST_ASKED_PER_TICK: usize = 8;

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

    // One reading for both phases, and the hostnames the first wrote for are
    // left out of the second: a decision made from this reading would not know
    // the revocation had passed the hostname on, and asking again would put it
    // back on Canopy and overwrite the reason. They are asked on the next tick,
    // which decides from what the revocation recorded.
    let Some(snapshot) = snapshot(state, &managed) else {
        return;
    };
    let replaced = replace_condemned(state, &engine, &snapshot, names).await;
    ask_canopy(state, &engine, &managed, &snapshot, &replaced).await;
}

/// Bring the Canopy faults into line with the hostnames' conditions now.
///
/// Run on every report tick, including ticks that reach no one: see the caller.
pub fn sync_all_faults(state: &Arc<OiState>) {
    let managed = {
        let registry = state.registry.read();
        state::managed_ingresses(&registry)
    };
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
    match state.db.call(move |db| Snapshot::load(db, env)) {
        Ok(snapshot) => Some(snapshot),
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
async fn replace_condemned<R: CertificateRelay>(
    state: &Arc<OiState>,
    engine: &Engine<R>,
    snap: &Snapshot,
    names: &Entitlements,
) -> BTreeSet<String> {
    let mut replaced = BTreeSet::new();
    let held = names.certificates.clone();
    let condemned = match state.db.call(move |db| canopy::condemned_by(db, &held)) {
        Ok(condemned) => condemned,
        Err(e) => {
            tracing::error!(error = %e, "canopy certificates: cannot compare held certificates");
            return replaced;
        }
    };
    for c in condemned {
        replaced.insert(c.hostname.clone());
        let st = state::compute_state(snap, &c.hostname);
        let ask = match &st.decision {
            Decision::Chain(chain) => revocation_ask(&c.hostname, chain, snap.now),
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
                    .revoke_and_reorder(ask.clone(), c.cert_id, c.reason.clone())
                    .await
                {
                    record_engine_failure(state, &ask, &e);
                }
            }
            // The hostname has left its chain: the certificate must still stop
            // being served, and nothing more can be said about what replaces it.
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
    replaced
}

/// What to say to Canopy for a hostname whose certificate it has revoked.
///
/// A revocation starts a cycle whenever it finds one not already running: the
/// certificate it condemns was the one serving the hostname, so the hostname has
/// something to replace and passes to the next strategy at once. Without this a
/// revocation before the renewal point would record nothing, and the hostname
/// would read as never having had a certificate and stay with Canopy.
// r[impl tls.canopy.revoked]
fn revocation_ask(hostname: &str, c: &ChainDecision, now: i64) -> Option<Ask> {
    match &c.cycle {
        chain::Cycle::Running(_) => ask_for(hostname, c, false),
        chain::Cycle::Idle { .. } => {
            let CanopyStanding::Applies { app_type } = &c.canopy else {
                return None;
            };
            Some(Ask {
                hostname: hostname.to_owned(),
                app_type: app_type.clone(),
                // The cycle starts now, with the condemned certificate as what
                // is being replaced: `apply_revocation` reads it from the row it
                // is given rather than from here.
                replacing: Identity::NONE,
                step: 0,
                since: now,
                chain_len: c.strategies.len(),
                canopy_responsible: true,
                trigger: AttemptTrigger::Renewal,
            })
        }
    }
}

async fn ask_canopy<R: CertificateRelay>(
    state: &Arc<OiState>,
    engine: &Engine<R>,
    managed: &[ManagedIngress],
    snap: &Snapshot,
    skip: &BTreeSet<String>,
) {
    let hostnames: BTreeSet<&str> = managed.iter().map(|m| m.hostname.as_str()).collect();
    let mut asked = 0;
    for hostname in hostnames {
        if skip.contains(hostname) {
            // A revocation wrote for this hostname after the reading below was
            // taken, so nothing here knows where it now stands.
            continue;
        }
        if asked >= MOST_ASKED_PER_TICK {
            // The rest are asked on the next tick: the decision is made again
            // from what is recorded, so nothing is lost by stopping here, and a
            // relay that answers slowly cannot make one tick run for the sum of
            // every hostname's wait while revocations and faults wait behind it.
            tracing::debug!(
                asked,
                "canopy certificates: the rest of the hostnames are left to the next tick"
            );
            break;
        }
        let st = state::compute_state(snap, hostname);
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
            let replacing = c.replacing();
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

        // The retry has just written the position the cycle starts from, so the
        // question to Canopy is built from what stands now and not from the
        // decision that was made before it.
        let fresh_snap;
        let fresh;
        let c = if forced {
            let Some(reloaded) = snapshot(state, managed) else {
                continue;
            };
            fresh_snap = reloaded;
            fresh = state::compute_state(&fresh_snap, hostname);
            match &fresh.decision {
                Decision::Chain(c) => c,
                _ => continue,
            }
        } else {
            c
        };
        let Some(ask) = ask_for(hostname, c, forced) else {
            continue;
        };
        asked += 1;
        match engine.ask(ask.clone()).await {
            Ok(outcome) => tracing::debug!(%hostname, ?outcome, "asked Canopy"),
            Err(e) => record_engine_failure(state, &ask, &e),
        }
    }
}

/// Record a failure that stopped the question being put at all.
///
/// An error here happened before Canopy answered anything, so nothing of the
/// answer path ran: without this the hostname would be asked again every tick
/// with nothing recorded, no cadence and no fault — a condition that may never
/// pass (a key that no longer decrypts) retried for ever and visible only in the
/// log. Recorded where every answer is recorded, so the cadence and the fault
/// follow from it.
// r[impl tls.canopy.fault]
fn record_engine_failure(state: &Arc<OiState>, ask: &Ask, error: &canopy::Error) {
    let reason = format!("the request to Canopy could not be made: {error}");
    tracing::error!(hostname = %ask.hostname, "{reason}");
    let hostname = ask.hostname.clone();
    let (identity, step, since) = (ask.replacing, ask.step, ask.since);
    let now = jiff::Timestamp::now().as_second();
    if let Err(e) = state.db.call(move |db| {
        let open_order_since = store::get_chain_row(db, &hostname)?
            .filter(|row| row.row.identity == identity)
            .and_then(|row| row.row.open_order_since);
        store::put_chain_row(
            db,
            &hostname,
            &chain::Row {
                identity,
                step,
                since,
                // Nothing was learnt about an order already open, so it stays
                // open; a fresh question that could not be put opens none.
                open_order_since,
                retry_not_before: Some(now + AUTO_RETRY_DEBOUNCE_SECS),
            },
            Some(&reason),
        )
    }) {
        tracing::error!(hostname = %ask.hostname, error = %e, "canopy certificates: cannot record why the request could not be made");
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::oi::test_support::TestOi;
    use crate::runtime::tls::{
        KeyType, TlsCertOrigin, TlsCertState, chain,
        store::{self, CertMetadata, NewCertificate},
    };

    const TYPED: &str = r#"
        app.canopy_type("tamanu");
        let web = app.service("web").exported().http(80);
        web.ingress("shop.example.com", 443).tls(Terminate.Https, Output.Http1);
        app.deployment("web").image("docker.io/library/nginx:1.29").http(80, web.route("/"));
    "#;

    const UNTYPED: &str = r#"
        let web = app.service("web").exported().http(80);
        web.ingress("shop.example.com", 443).tls(Terminate.Https, Output.Http1);
        app.deployment("web").image("docker.io/library/nginx:1.29").http(80, web.route("/"));
    "#;

    fn shop(oi: &TestOi, script: &str) -> Vec<ManagedIngress> {
        oi.call("/apps/create", json!({ "app": "shop", "script": script }))
            .unwrap();
        oi.install("shop");
        let registry = oi.state.registry.read();
        state::managed_ingresses(&registry)
    }

    fn active_faults(oi: &TestOi) -> Vec<faults::FaultRecord> {
        oi.state.db.call(|db| {
            faults::list_active_faults(db, None)
                .unwrap()
                .into_iter()
                .filter(|f| f.kind == FAULT_KIND)
                .collect()
        })
    }

    fn record_failure(oi: &TestOi, reason: &str) {
        let reason = reason.to_owned();
        oi.state.db.call(move |db| {
            store::put_chain_row(
                db,
                "shop.example.com",
                &chain::Row {
                    identity: chain::Identity::NONE,
                    step: 0,
                    since: 0,
                    open_order_since: None,
                    retry_not_before: None,
                },
                Some(&reason),
            )
            .unwrap();
        });
    }

    // r[verify tls.canopy-only.untyped]
    // r[verify tls.canopy.fault]
    #[test]
    fn a_canopy_only_hostname_nothing_can_ask_about_is_faulted_until_it_can_be() {
        let oi = TestOi::new();
        let managed = shop(&oi, UNTYPED);
        oi.call(
            "/tls/policies/set-canopy",
            json!({ "hostname": "shop.example.com" }),
        )
        .unwrap();

        oi.block_on(async { sync_faults(&oi.state, &managed) });
        let filed = active_faults(&oi);
        assert_eq!(filed.len(), 1);
        assert_eq!(
            filed[0].resource_name.as_deref(),
            Some("shop.example.com:443")
        );
        assert!(
            filed[0]
                .description
                .contains("no app declaring it has a Canopy type"),
            "{}",
            filed[0].description
        );

        // Filing again is not filing twice.
        oi.block_on(async { sync_faults(&oi.state, &managed) });
        assert_eq!(active_faults(&oi).len(), 1);

        oi.call(
            "/tls/policies/clear",
            json!({ "hostname": "shop.example.com" }),
        )
        .unwrap();
        oi.block_on(async { sync_faults(&oi.state, &managed) });
        assert!(
            active_faults(&oi).is_empty(),
            "clearing the policy is one of the ways the fault goes"
        );
    }

    // r[verify tls.canopy.fault]
    #[test]
    fn a_hostname_with_no_certificate_whose_request_keeps_failing_is_faulted() {
        let oi = TestOi::new();
        let managed = shop(&oi, TYPED);
        oi.call(
            "/tls/policies/set-canopy",
            json!({ "hostname": "shop.example.com" }),
        )
        .unwrap();
        record_failure(&oi, "dns challenge timed out");

        oi.block_on(async { sync_faults(&oi.state, &managed) });
        let filed = active_faults(&oi);
        assert_eq!(filed.len(), 1);
        assert!(
            filed[0].description.contains("dns challenge timed out"),
            "{}",
            filed[0].description
        );

        // A usable certificate arrives: the chain has done its job.
        let now = jiff::Timestamp::now().as_second();
        let pem = crate::runtime::tls::test_support::self_signed_pem(&["shop.example.com"]);
        oi.state.db.call(move |db| {
            store::insert_certificate(
                db,
                NewCertificate {
                    hostname: "shop.example.com",
                    requested_hostname: None,
                    state: TlsCertState::Active,
                    origin: TlsCertOrigin::Canopy,
                    cert_pem: Some(&pem),
                    csr_pem: None,
                    key_ciphertext: &[0u8; 8],
                    key_type: KeyType::EcdsaP256,
                    metadata: CertMetadata {
                        not_before: Some(now - 86_400),
                        not_after: Some(now + 89 * 86_400),
                        ..Default::default()
                    },
                    note: None,
                    acme_account_id: None,
                },
            )
            .unwrap();
        });
        oi.block_on(async { sync_faults(&oi.state, &managed) });
        assert!(active_faults(&oi).is_empty());
    }

    // r[verify tls.canopy.fault]
    #[test]
    fn trouble_while_a_certificate_is_still_usable_files_nothing() {
        let oi = TestOi::new();
        let managed = shop(&oi, TYPED);
        let now = jiff::Timestamp::now().as_second();
        let pem = crate::runtime::tls::test_support::self_signed_pem(&["shop.example.com"]);
        oi.state.db.call(move |db| {
            store::insert_certificate(
                db,
                NewCertificate {
                    hostname: "shop.example.com",
                    requested_hostname: None,
                    state: TlsCertState::Active,
                    origin: TlsCertOrigin::Canopy,
                    cert_pem: Some(&pem),
                    csr_pem: None,
                    key_ciphertext: &[0u8; 8],
                    key_type: KeyType::EcdsaP256,
                    metadata: CertMetadata {
                        // Past its renewal point and inside Canopy's window.
                        not_before: Some(now - 70 * 86_400),
                        not_after: Some(now + 20 * 86_400),
                        ..Default::default()
                    },
                    note: None,
                    acme_account_id: None,
                },
            )
            .unwrap();
        });
        record_failure(&oi, "canopy is having a bad day");
        oi.block_on(async { sync_faults(&oi.state, &managed) });
        assert!(
            active_faults(&oi).is_empty(),
            "the certificate still serves and the chain is still working on it"
        );
    }

    // r[verify tls.canopy.fault]
    #[test]
    fn a_chain_that_ran_out_of_strategies_without_a_replacement_is_faulted() {
        let oi = TestOi::new();
        let managed = shop(&oi, TYPED);
        let now = jiff::Timestamp::now().as_second();
        let pem = crate::runtime::tls::test_support::self_signed_pem(&["shop.example.com"]);
        oi.state.db.call(move |db| {
            store::insert_certificate(
                db,
                NewCertificate {
                    hostname: "shop.example.com",
                    requested_hostname: None,
                    state: TlsCertState::Active,
                    origin: TlsCertOrigin::Canopy,
                    cert_pem: Some(&pem),
                    csr_pem: None,
                    key_ciphertext: &[0u8; 8],
                    key_type: KeyType::EcdsaP256,
                    metadata: CertMetadata {
                        // Past the point Canopy falls through at.
                        not_before: Some(now - 80 * 86_400),
                        not_after: Some(now + 10 * 86_400),
                        ..Default::default()
                    },
                    note: None,
                    acme_account_id: None,
                },
            )
            .unwrap();
        });
        oi.block_on(async { sync_faults(&oi.state, &managed) });
        let filed = active_faults(&oi);
        assert_eq!(filed.len(), 1);
        assert!(
            filed[0].description.contains("fell through to acme_http01"),
            "{}",
            filed[0].description
        );
    }

    /// A relay that records what it was asked and answers that the order is open.
    #[derive(Default)]
    struct Recorder(parking_lot::Mutex<Vec<(String, Option<String>)>>);

    impl CertificateRelay for &Recorder {
        async fn request(
            &self,
            name: &str,
            _csr_der_b64: &str,
            app_type: Option<&str>,
        ) -> Result<bestool_canopy::schema::CertificateResponse, canopy::RelayFailure> {
            self.0
                .lock()
                .push((name.to_owned(), app_type.map(str::to_owned)));
            Ok(bestool_canopy::schema::CertificateResponse::builder()
                .key_must_be_replaced(false)
                .name(name.to_owned())
                .revoked(false)
                .state("pending".to_owned())
                .usable(false)
                .build())
        }
    }

    fn script_for(app_type: &str, hostname: &str, port: u16) -> String {
        format!(
            r#"
            app.canopy_type("{app_type}");
            let web = app.service("web").exported().http(80);
            web.ingress("{hostname}", {port}).tls(Terminate.Https, Output.Http1);
            app.deployment("web").image("docker.io/library/nginx:1.29").http(80, web.route("/"));
            "#
        )
    }

    fn register(oi: &TestOi, app: &str, script: &str) {
        oi.call("/apps/create", json!({ "app": app, "script": script }))
            .unwrap();
        oi.install(app);
    }

    fn managed_in(oi: &TestOi) -> Vec<ManagedIngress> {
        let registry = oi.state.registry.read();
        state::managed_ingresses(&registry)
    }

    fn engine<'a>(oi: &TestOi, recorder: &'a Recorder) -> Engine<&'a Recorder> {
        Engine::new(oi.state.db.clone(), Arc::clone(&oi.state.cipher), recorder)
    }

    /// Ask Canopy about every due hostname, against the state as it stands.
    async fn asking(oi: &TestOi, recorder: &Recorder, managed: &[ManagedIngress]) {
        let snap = snapshot(&oi.state, managed).expect("the state can be read");
        ask_canopy(
            &oi.state,
            &engine(oi, recorder),
            managed,
            &snap,
            &BTreeSet::new(),
        )
        .await;
    }

    // r[verify tls.canopy.request]
    #[test]
    fn one_request_is_made_for_a_hostname_however_many_apps_declare_it() {
        let oi = TestOi::new();
        register(&oi, "shop", &script_for("tamanu", "shop.example.com", 443));
        register(
            &oi,
            "shop2",
            &script_for("tamanu", "shop.example.com", 8443),
        );
        oi.call(
            "/tls/policies/set-canopy",
            json!({ "hostname": "shop.example.com" }),
        )
        .unwrap();

        let recorder = Recorder::default();
        let managed = managed_in(&oi);
        oi.block_on(asking(&oi, &recorder, &managed));
        let asked = recorder.0.lock().clone();
        assert_eq!(
            asked,
            vec![("shop.example.com".to_owned(), Some("tamanu".to_owned()))],
            "one request, naming the type both apps share"
        );
    }

    // r[verify tls.canopy.request]
    #[test]
    fn apps_of_different_types_on_one_hostname_send_no_type() {
        let oi = TestOi::new();
        register(&oi, "shop", &script_for("tamanu", "shop.example.com", 443));
        register(&oi, "shop2", &script_for("other", "shop.example.com", 8443));
        oi.call(
            "/tls/policies/set-canopy",
            json!({ "hostname": "shop.example.com" }),
        )
        .unwrap();

        let recorder = Recorder::default();
        let managed = managed_in(&oi);
        oi.block_on(asking(&oi, &recorder, &managed));
        assert_eq!(
            recorder.0.lock().clone(),
            vec![("shop.example.com".to_owned(), None)]
        );
    }

    // r[verify tls.canopy.request]
    // r[verify tls.policy.wildcard]
    #[test]
    fn a_wildcard_policy_asks_for_each_concrete_hostname_and_never_the_wildcard() {
        let oi = TestOi::new();
        register(&oi, "one", &script_for("tamanu", "a.example.com", 443));
        register(&oi, "two", &script_for("tamanu", "b.example.com", 443));
        oi.call(
            "/tls/policies/set-canopy",
            json!({ "hostname": "*.example.com" }),
        )
        .unwrap();

        let recorder = Recorder::default();
        let managed = managed_in(&oi);
        oi.block_on(asking(&oi, &recorder, &managed));
        let mut names: Vec<String> = recorder.0.lock().iter().map(|(n, _)| n.clone()).collect();
        names.sort();
        assert_eq!(names, ["a.example.com", "b.example.com"]);
    }

    // r[verify tls.strategy.default]
    // r[verify tls.chain.no-certificate]
    #[test]
    fn nothing_is_asked_of_canopy_for_a_hostname_the_proxy_alone_serves() {
        let oi = TestOi::new();
        register(&oi, "plain", UNTYPED);
        let recorder = Recorder::default();
        let managed = managed_in(&oi);
        oi.block_on(asking(&oi, &recorder, &managed));
        assert!(recorder.0.lock().is_empty());

        // And a typed one with no offer goes to the proxy without waiting.
        let oi = TestOi::new();
        register(&oi, "shop", &script_for("tamanu", "shop.example.com", 443));
        let managed = managed_in(&oi);
        oi.block_on(asking(&oi, &recorder, &managed));
        assert!(recorder.0.lock().is_empty());
    }

    // r[verify tls.cert.force-retry]
    // r[verify tls.chain.return]
    #[test]
    fn an_operator_retry_asks_canopy_again_for_a_hostname_a_refusal_moved_on() {
        let oi = TestOi::new();
        register(&oi, "shop", &script_for("tamanu", "shop.example.com", 443));

        // A manual certificate past its renewal point, and a recorded refusal
        // that handed the hostname on to HTTP-01.
        let now = jiff::Timestamp::now().as_second();
        let pem = crate::runtime::tls::test_support::self_signed_pem(&["shop.example.com"]);
        let (id, not_after) = (now + 20 * 86_400, now + 20 * 86_400);
        let cert_id = oi.state.db.call(move |db| {
            let cert_id = store::insert_certificate(
                db,
                NewCertificate {
                    hostname: "shop.example.com",
                    requested_hostname: None,
                    state: TlsCertState::Active,
                    origin: TlsCertOrigin::Manual,
                    cert_pem: Some(&pem),
                    csr_pem: None,
                    key_ciphertext: &[0u8; 8],
                    key_type: KeyType::EcdsaP256,
                    metadata: CertMetadata {
                        not_before: Some(now - 70 * 86_400),
                        not_after: Some(id),
                        ..Default::default()
                    },
                    note: None,
                    acme_account_id: None,
                },
            )
            .unwrap();
            store::put_chain_row(
                db,
                "shop.example.com",
                &chain::Row {
                    identity: chain::Identity {
                        cert_id: Some(cert_id),
                        not_after: Some(not_after),
                    },
                    step: 1,
                    since: now - 86_400,
                    open_order_since: None,
                    retry_not_before: Some(now + 3_600),
                },
                Some("refused"),
            )
            .unwrap();
            store::set_force_retry(db, "shop.example.com").unwrap();
            cert_id
        });

        let recorder = Recorder::default();
        let managed = managed_in(&oi);
        oi.block_on(asking(&oi, &recorder, &managed));

        assert_eq!(
            recorder.0.lock().len(),
            1,
            "the retry asks Canopy again although the refusal had moved the hostname on"
        );
        let (row, queued) = oi.state.db.call(|db| {
            (
                store::get_chain_row(db, "shop.example.com")
                    .unwrap()
                    .unwrap(),
                store::list_force_retries(db).unwrap().len(),
            )
        });
        assert_eq!(queued, 0, "the retry was consumed");
        assert_eq!(row.row.identity.cert_id, Some(cert_id));
        assert_eq!(
            row.row.step, 0,
            "the cycle restarted from the first strategy"
        );
        assert!(row.row.open_order_since.is_some());
    }

    // r[verify tls.canopy-only.untyped]
    // r[verify tls.canopy.fault]
    #[test]
    fn the_faults_follow_the_hostname_not_the_relay() {
        let oi = TestOi::new();
        let managed = shop(&oi, UNTYPED);
        oi.call(
            "/tls/policies/set-canopy",
            json!({ "hostname": "shop.example.com" }),
        )
        .unwrap();
        oi.block_on(async { sync_faults(&oi.state, &managed) });
        assert_eq!(active_faults(&oi).len(), 1);

        // No offer is live, so the report reaches no one; clearing the policy
        // must still clear the fault.
        oi.call(
            "/tls/policies/clear",
            json!({ "hostname": "shop.example.com" }),
        )
        .unwrap();
        oi.block_on(async {
            let _ = crate::runtime::canopy::report(&oi.state).await;
        });
        assert!(active_faults(&oi).is_empty());
    }

    /// Store a Canopy certificate serving `hostname`, valid for the window given.
    fn canopy_cert(oi: &TestOi, hostname: &str, not_before: i64, not_after: i64) -> i64 {
        let pem = crate::runtime::tls::test_support::self_signed_pem(&[hostname]);
        let hostname = hostname.to_owned();
        oi.state.db.call(move |db| {
            store::insert_certificate(
                db,
                NewCertificate {
                    hostname: &hostname,
                    requested_hostname: None,
                    state: TlsCertState::Active,
                    origin: TlsCertOrigin::Canopy,
                    cert_pem: Some(&pem),
                    csr_pem: None,
                    key_ciphertext: &[0u8; 8],
                    key_type: KeyType::EcdsaP256,
                    metadata: CertMetadata {
                        not_before: Some(not_before),
                        not_after: Some(not_after),
                        ..Default::default()
                    },
                    note: None,
                    acme_account_id: None,
                },
            )
            .unwrap()
        })
    }

    // r[verify tls.canopy.revoked]
    // r[verify tls.chain.no-certificate]
    #[test]
    fn a_revocation_before_the_renewal_point_still_passes_the_hostname_on() {
        let oi = TestOi::new();
        register(&oi, "shop", &script_for("tamanu", "shop.example.com", 443));
        let now = jiff::Timestamp::now().as_second();
        // Well before its renewal point, so the cycle is idle.
        let cert_id = canopy_cert(&oi, "shop.example.com", now - 86_400, now + 89 * 86_400);

        let recorder = Recorder::default();
        let managed = managed_in(&oi);
        let names = Entitlements::builder()
            .certificates(vec![
                bestool_canopy::schema::HeldCertificate::builder()
                    .key_fingerprint(canopy_key_fingerprint(&oi, cert_id))
                    .key_must_be_replaced(false)
                    .name("shop.example.com".to_owned())
                    .revoked(true)
                    .usable(false)
                    .build(),
            ])
            .applications(vec![])
            .certificate_names(vec![])
            .domains(vec![])
            .may_manage_dns(false)
            .may_manage_tls(true)
            .paused(false)
            .registered_names(vec![])
            .build();
        oi.block_on(async {
            let snap = snapshot(&oi.state, &managed).expect("the state can be read");
            replace_condemned(&oi.state, &engine(&oi, &recorder), &snap, &names).await;
        });

        let (row, revoked) = oi.state.db.call(move |db| {
            (
                store::get_chain_row(db, "shop.example.com").unwrap(),
                store::get_certificate(db, cert_id).unwrap().unwrap().state,
            )
        });
        assert_eq!(revoked, TlsCertState::Revoked);
        let row = row.expect("a revocation records what the hostname is replacing");
        assert_eq!(
            row.row.identity.cert_id,
            Some(cert_id),
            "the revoked certificate is the one being replaced"
        );
        assert_eq!(
            row.row.step, 1,
            "and the hostname passes to the next strategy at once"
        );
        assert_eq!(recorder.0.lock().len(), 1, "a replacement was asked for");
    }

    /// The fingerprint Canopy names a stored certificate's key by.
    fn canopy_key_fingerprint(oi: &TestOi, cert_id: i64) -> String {
        let pem = oi
            .state
            .db
            .call(move |db| store::get_certificate(db, cert_id).unwrap().unwrap())
            .cert_pem
            .unwrap();
        let spki = crate::runtime::tls::parse::leaf_spki_der(&pem).unwrap();
        crate::runtime::tls::canopy::key_fingerprint(&spki)
    }

    /// A relay that is never reached, because the question cannot be put.
    struct Broken;
    impl CertificateRelay for &Broken {
        async fn request(
            &self,
            _name: &str,
            _csr_der_b64: &str,
            _app_type: Option<&str>,
        ) -> Result<bestool_canopy::schema::CertificateResponse, canopy::RelayFailure> {
            unreachable!("the engine fails before the relay is asked")
        }
    }

    // r[verify tls.canopy.fault]
    #[test]
    fn a_failure_before_the_question_is_put_is_recorded_and_faulted() {
        let oi = TestOi::new();
        let managed = shop(&oi, TYPED);
        oi.call(
            "/tls/policies/set-canopy",
            json!({ "hostname": "shop.example.com" }),
        )
        .unwrap();

        // A key that no longer decrypts: the engine fails before it can ask.
        oi.state.db.call(|db| {
            store::insert_certificate(
                db,
                NewCertificate {
                    hostname: "shop.example.com",
                    requested_hostname: Some("shop.example.com"),
                    state: TlsCertState::CsrPending,
                    origin: TlsCertOrigin::Canopy,
                    cert_pem: None,
                    csr_pem: Some("not a signing request"),
                    key_ciphertext: b"not a key",
                    key_type: KeyType::EcdsaP256,
                    metadata: CertMetadata::default(),
                    note: None,
                    acme_account_id: None,
                },
            )
            .unwrap();
        });

        let engine = Engine::new(oi.state.db.clone(), Arc::clone(&oi.state.cipher), &Broken);
        oi.block_on(async {
            let snap = snapshot(&oi.state, &managed).expect("the state can be read");
            ask_canopy(&oi.state, &engine, &managed, &snap, &BTreeSet::new()).await;
        });

        let row = oi
            .state
            .db
            .call(|db| store::get_chain_row(db, "shop.example.com").unwrap())
            .expect("the failure is recorded where an answer would be");
        assert!(
            row.last_error.unwrap().contains("could not be made"),
            "so the operator sees why"
        );
        assert!(
            row.row.retry_not_before.is_some(),
            "and it is not asked again every tick"
        );

        oi.block_on(async { sync_faults(&oi.state, &managed) });
        assert_eq!(
            active_faults(&oi).len(),
            1,
            "a condition that may never pass is faulted rather than retried in silence"
        );
    }

    // r[verify tls.canopy.request]
    #[test]
    fn one_tick_asks_about_no_more_hostnames_than_it_can_wait_for() {
        let oi = TestOi::new();
        for i in 0..(MOST_ASKED_PER_TICK + 3) {
            register(
                &oi,
                &format!("app{i}"),
                &script_for("tamanu", &format!("h{i}.example.com"), 443),
            );
        }
        oi.call(
            "/tls/policies/set-canopy",
            json!({ "hostname": "*.example.com" }),
        )
        .unwrap();

        let recorder = Recorder::default();
        let managed = managed_in(&oi);
        oi.block_on(asking(&oi, &recorder, &managed));
        assert_eq!(recorder.0.lock().len(), MOST_ASKED_PER_TICK);
    }

    // r[verify tls.canopy.revoked]
    // r[verify tls.chain.return]
    #[test]
    fn a_hostname_a_revocation_touched_is_not_asked_again_in_the_same_tick() {
        let oi = TestOi::new();
        register(&oi, "shop", &script_for("tamanu", "shop.example.com", 443));
        let now = jiff::Timestamp::now().as_second();
        // Past its renewal point, so without the guard the second phase would
        // decide the hostname is Canopy's to ask about again.
        let cert_id = canopy_cert(
            &oi,
            "shop.example.com",
            now - 70 * 86_400,
            now + 20 * 86_400,
        );

        let recorder = Recorder::default();
        let managed = managed_in(&oi);
        let names = revoking(&oi, cert_id, "shop.example.com");
        oi.block_on(async {
            let snap = snapshot(&oi.state, &managed).expect("the state can be read");
            let engine = engine(&oi, &recorder);
            let replaced = replace_condemned(&oi.state, &engine, &snap, &names).await;
            assert!(replaced.contains("shop.example.com"));
            ask_canopy(&oi.state, &engine, &managed, &snap, &replaced).await;
        });

        assert_eq!(
            recorder.0.lock().len(),
            1,
            "the replacement was asked for once, and not asked again from the reading before it"
        );
        let row = oi
            .state
            .db
            .call(|db| store::get_chain_row(db, "shop.example.com").unwrap())
            .expect("the revocation recorded where the hostname stands");
        assert_eq!(
            row.row.step, 1,
            "and it is still passed to the next strategy"
        );
        assert!(
            row.row.open_order_since.is_some(),
            "with the one replacement order the revocation opened"
        );
    }

    /// What Canopy says of a certificate it has revoked.
    fn revoking(oi: &TestOi, cert_id: i64, hostname: &str) -> Entitlements {
        Entitlements::builder()
            .applications(vec![])
            .certificate_names(vec![])
            .certificates(vec![
                bestool_canopy::schema::HeldCertificate::builder()
                    .key_fingerprint(canopy_key_fingerprint(oi, cert_id))
                    .key_must_be_replaced(false)
                    .name(hostname.to_owned())
                    .revoked(true)
                    .usable(false)
                    .build(),
            ])
            .domains(vec![])
            .may_manage_dns(false)
            .may_manage_tls(true)
            .paused(false)
            .registered_names(vec![])
            .build()
    }
}
