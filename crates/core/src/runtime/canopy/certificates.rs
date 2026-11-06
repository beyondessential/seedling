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
            canopy::{self, Ask, CanopyRelay, CertificateRelay, Engine},
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
async fn replace_condemned<R: CertificateRelay>(
    state: &Arc<OiState>,
    engine: &Engine<R>,
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

async fn ask_canopy<R: CertificateRelay>(
    state: &Arc<OiState>,
    engine: &Engine<R>,
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
        oi.block_on(ask_canopy(&oi.state, &engine(&oi, &recorder), &managed));
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
        oi.block_on(ask_canopy(&oi.state, &engine(&oi, &recorder), &managed));
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
        oi.block_on(ask_canopy(&oi.state, &engine(&oi, &recorder), &managed));
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
        oi.block_on(ask_canopy(&oi.state, &engine(&oi, &recorder), &managed));
        assert!(recorder.0.lock().is_empty());

        // And a typed one with no offer goes to the proxy without waiting.
        let oi = TestOi::new();
        register(&oi, "shop", &script_for("tamanu", "shop.example.com", 443));
        let managed = managed_in(&oi);
        oi.block_on(ask_canopy(&oi.state, &engine(&oi, &recorder), &managed));
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
        oi.block_on(ask_canopy(&oi.state, &engine(&oi, &recorder), &managed));

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
}
