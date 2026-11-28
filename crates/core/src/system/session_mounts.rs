//! The mount translations of live shell sessions.
//!
//! A shell session's job mounts services as a pod does, but its container
//! starts at once, without waiting for the reconciler to apply rules that
//! include it. Its translations are built from the backends of the last rules
//! the reconciler applied, held by the data plane apart from those rules, and
//! rebuilt whenever the reconciler applies rules again.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use ipnet::Ipv6Net;
use seedling_protocol::names::AppName;

use crate::{
    defs::service::ServicePort,
    runtime::db::DbHandle,
    system::{
        BoxError, ContainerRuntime, DataPlane,
        reconcile::rules::{BackendView, mount_rules},
        types::MountRule,
    },
};

/// What a shell session mounts, and from where.
#[derive(Debug, Clone)]
pub struct SessionMount {
    /// The app whose services the session's job mounts.
    pub app: AppName,
    /// The session's pod network prefix.
    pub prefix: Ipv6Net,
    pub mounts: Vec<ServicePort>,
}

struct Live {
    mount: SessionMount,
    /// The translations the data plane holds for the session.
    held: Vec<MountRule>,
    /// Whether rebuilding the translations from the latest backends failed,
    /// leaving the session on older ones.
    behind: bool,
}

#[derive(Default)]
struct State {
    /// The backends of the rules the reconciler last applied, or `None` before
    /// it has applied any.
    backends: Option<Arc<BackendView>>,
    live: BTreeMap<String, Live>,
    /// Sessions that have ended but whose translations the data plane failed
    /// to remove. Their pod network prefixes stay allocated until the
    /// translations are gone, so no other pod network is given a prefix that
    /// stale translations still match.
    withdrawn: BTreeSet<String>,
    /// The last failure to write the translations.
    last_failure: Option<String>,
}

impl State {
    fn failure(&self) -> Option<String> {
        let failing = !self.withdrawn.is_empty()
            || self.live.values().any(|live| live.behind);
        failing.then(|| {
            self.last_failure
                .clone()
                .unwrap_or_else(|| "the reason was not recorded".to_owned())
        })
    }

    fn rules_for(&self, mount: &SessionMount) -> Vec<MountRule> {
        self.backends
            .as_deref()
            .map(|backends| mount_rules(backends, &mount.app, mount.prefix, &mount.mounts))
            .unwrap_or_default()
    }

    /// Record that a write replaced every session's translations: nothing an
    /// earlier failed write left behind remains, so the prefixes held back for
    /// withdrawn sessions are freed.
    fn replaced(&mut self, db: &DbHandle) {
        for net_name in std::mem::take(&mut self.withdrawn) {
            release_prefix(db, &net_name);
        }
    }

    fn failed(&mut self, what: &str, e: &BoxError) {
        tracing::error!(error = %e, "{what} failed");
        self.last_failure = Some(format!("{what} failed: {e}"));
    }
}

/// The mount translations of live shell sessions, keyed by each session's pod
/// network name.
#[derive(Default)]
pub struct SessionMounts {
    // Held across each write to the data plane, so the translations it holds
    // follow the order of the changes made here.
    state: tokio::sync::Mutex<State>,
    failure: parking_lot::Mutex<Option<String>>,
}

impl SessionMounts {
    /// Why the data plane's session translations differ from what the live
    /// sessions need, or `None` while they do not.
    pub fn failure(&self) -> Option<String> {
        self.failure.lock().clone()
    }

    fn settle(&self, state: &State) {
        *self.failure.lock() = state.failure();
    }

    /// Give the session on `net_name` the translations of `mount`, built from
    /// the backends of the last applied rules; before any rules are applied it
    /// holds none, and gains them with the first apply. A session that mounts
    /// nothing is given none and is not registered. On failure the session is
    /// not registered and holds no translations.
    // spec: INFRA#mount-endpoint-addressing
    pub async fn register(
        &self,
        data_plane: &dyn DataPlane,
        db: &DbHandle,
        net_name: &str,
        mount: SessionMount,
    ) -> Result<(), BoxError> {
        if mount.mounts.is_empty() {
            return Ok(());
        }
        let mut state = self.state.lock().await;
        let rules = state.rules_for(&mount);
        data_plane.set_session_mounts(net_name, &rules).await?;
        state.replaced(db);
        state.live.insert(
            net_name.to_owned(),
            Live {
                mount,
                held: rules,
                behind: false,
            },
        );
        self.settle(&state);
        Ok(())
    }

    /// Remove the translations of the session on `net_name`. Returns whether
    /// it holds none, so its prefix may be freed: a session never registered
    /// holds none. One whose translations could not be removed is withdrawn,
    /// and its prefix is freed once a later write replaces them.
    // spec: INFRA#mount-endpoint-addressing
    async fn deregister(&self, data_plane: &dyn DataPlane, db: &DbHandle, net_name: &str) -> bool {
        let mut state = self.state.lock().await;
        if state.live.remove(net_name).is_none() {
            return true;
        }
        let removed = match data_plane.remove_session_mounts(net_name).await {
            Ok(()) => {
                state.replaced(db);
                true
            }
            Err(e) => {
                state.failed(
                    &format!("removing the mount translations of the shell session on {net_name}"),
                    &e,
                );
                state.withdrawn.insert(net_name.to_owned());
                false
            }
        };
        self.settle(&state);
        removed
    }

    /// Remove a session's pod network and its translations, and free its
    /// prefix once no translation matches it.
    // spec: INFRA#pod-networks
    // spec: INFRA#mount-endpoint-addressing
    pub async fn remove_session_network(
        &self,
        container: &dyn ContainerRuntime,
        data_plane: &dyn DataPlane,
        db: &DbHandle,
        net_name: &str,
    ) {
        let _ = container.remove_network(net_name).await;
        if self.deregister(data_plane, db, net_name).await {
            release_prefix(db, net_name);
        } else {
            tracing::warn!(
                %net_name,
                "the shell session's pod network prefix stays allocated until its mount \
                 translations are removed"
            );
        }
    }

    /// Take `backends`, those of rules just applied, and rebuild every live
    /// session's translations from them. Translations a failed write left
    /// behind are replaced too.
    // spec: INFRA#mount-endpoint-addressing
    pub(crate) async fn follow(
        &self,
        data_plane: &dyn DataPlane,
        db: &DbHandle,
        backends: Arc<BackendView>,
    ) {
        let mut state = self.state.lock().await;
        state.backends = Some(backends);
        let names: Vec<String> = state.live.keys().cloned().collect();
        for net_name in names {
            let Some(live) = state.live.get(&net_name) else {
                continue;
            };
            let rules = state.rules_for(&live.mount);
            if rules == live.held && !live.behind {
                continue;
            }
            match data_plane.set_session_mounts(&net_name, &rules).await {
                Ok(()) => {
                    state.replaced(db);
                    if let Some(live) = state.live.get_mut(&net_name) {
                        live.held = rules;
                        live.behind = false;
                    }
                }
                Err(e) => {
                    state.failed(
                        &format!(
                            "updating the mount translations of the shell session on {net_name}"
                        ),
                        &e,
                    );
                    if let Some(live) = state.live.get_mut(&net_name) {
                        live.behind = true;
                    }
                }
            }
        }
        if !state.withdrawn.is_empty() {
            let rewritten = match state.live.iter().next() {
                Some((net_name, live)) => data_plane.set_session_mounts(net_name, &live.held).await,
                None => data_plane.reset_sessions().await,
            };
            match rewritten {
                Ok(()) => state.replaced(db),
                Err(e) => state.failed("removing stale shell session mount translations", &e),
            }
        }
        self.settle(&state);
    }
}

/// Free the prefix of the session pod network `net_name`.
// spec: INFRA#pod-networks
fn release_prefix(db: &DbHandle, net_name: &str) {
    let name = net_name.to_owned();
    if let Err(e) = db.call(move |db| crate::runtime::pod_networks::release(db, &name)) {
        tracing::warn!(%net_name, "cannot free a pod network prefix: {e}");
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::{
        defs::resource::{Resource, ResourceKind},
        runtime::site_services::resolve::{EmptyLookup, ResolveCtx},
        system::{
            stub::{SessionCall, StubDataPlane},
            types::ForwardProto,
        },
    };

    const NET: &str = "seedling-demo-tools-1";

    fn app() -> AppName {
        AppName::new_unchecked("demo")
    }

    /// The `tools` job of a `demo` app, declared with `mount` appended, which
    /// can mount the `database` service at port 5432.
    pub(crate) fn tools_job(mount: &str) -> crate::defs::job::JobDef {
        let def = crate::tests::run_test_script_app(&format!(
            r#"
            let db = app.service("database");
            app.deployment("postgres").image("docker.io/library/postgres:16").tcp(5432, db.port(5432));
            app.job("tools").image("docker.io/library/busybox:1.37"){mount};
            "#
        ))
        .def
        .load_full();
        let Some(Resource::Job(job)) = def
            .resources
            .iter()
            .find(|(id, _)| id.kind == ResourceKind::Job)
            .map(|(_, r)| r.clone())
        else {
            panic!("the job is declared");
        };
        job.def.lock().clone()
    }

    /// The service ports the `tools` job mounts.
    pub(crate) fn tools_mounts(mount: &str) -> Vec<ServicePort> {
        tools_job(mount).pod.lock().service_mounts.clone()
    }

    fn session(mount: &str) -> SessionMount {
        SessionMount {
            app: app(),
            prefix: "fd5e:ed11:9000:5001::/64".parse().unwrap(),
            mounts: tools_mounts(mount),
        }
    }

    /// Backends of the `demo` app's `database` service at port 5432 over TCP.
    pub(crate) fn backends(addrs: &[&str]) -> Arc<BackendView> {
        let pool = addrs
            .iter()
            .map(|addr| (addr.parse().unwrap(), 5432))
            .collect();
        let by_app = HashMap::from([(
            app(),
            HashMap::from([(("database".to_owned(), 5432, ForwardProto::Tcp), pool)]),
        )]);
        Arc::new(BackendView::new(
            by_app,
            None,
            &ResolveCtx {
                nat64_active: false,
                has_ipv6_egress: true,
                resolver: &EmptyLookup,
            },
        ))
    }

    fn held_backends(plane: &StubDataPlane) -> Vec<Vec<(std::net::Ipv6Addr, u16)>> {
        plane
            .session_mounts()
            .get(NET)
            .map(|rules| rules.iter().map(|r| r.backends.clone()).collect())
            .unwrap_or_default()
    }

    fn allocated(db: &DbHandle) {
        db.call(|db| {
            crate::runtime::pod_networks::allocated_prefix(
                db,
                &"fd5e:ed11:9000::/48".parse().unwrap(),
                NET,
                ResourceKind::Job,
            )
        })
        .unwrap();
    }

    fn prefix_held(db: &DbHandle) -> bool {
        db.call(|db| {
            crate::runtime::pod_networks::recorded_prefix(
                db,
                &"fd5e:ed11:9000::/48".parse().unwrap(),
                NET,
                ResourceKind::Job,
            )
        })
        .unwrap()
        .is_some()
    }

    // spec: INFRA#mount-endpoint-addressing
    #[tokio::test]
    async fn a_live_sessions_translations_follow_the_backends_of_each_apply() {
        let plane = StubDataPlane::default();
        let db = DbHandle::open_in_memory().unwrap();
        let mounts = SessionMounts::default();
        mounts
            .follow(&plane, &db, backends(&["fd5e:ed11:9000:4001::2"]))
            .await;

        mounts
            .register(&plane, &db, NET, session(".mount(db.port(5432))"))
            .await
            .unwrap();
        let rules = &plane.session_mounts()[NET];
        assert_eq!(rules.len(), 1, "{rules:?}");
        assert_eq!(rules[0].pod_prefix, session("").prefix);
        assert_eq!(rules[0].mount_port, 5432);
        assert_eq!(rules[0].proto, ForwardProto::Tcp);
        assert_eq!(
            held_backends(&plane),
            [vec![("fd5e:ed11:9000:4001::2".parse().unwrap(), 5432)]]
        );

        mounts
            .follow(&plane, &db, backends(&["fd5e:ed11:9000:4002::2"]))
            .await;
        assert_eq!(
            held_backends(&plane),
            [vec![("fd5e:ed11:9000:4002::2".parse().unwrap(), 5432)]],
            "the session reaches the backend the apply routed to"
        );

        let calls = plane.session_calls().len();
        mounts
            .follow(&plane, &db, backends(&["fd5e:ed11:9000:4002::2"]))
            .await;
        assert_eq!(
            plane.session_calls().len(),
            calls,
            "unchanged backends rewrite nothing"
        );
        assert_eq!(mounts.failure(), None);
    }

    // spec: INFRA#mount-endpoint-addressing
    #[tokio::test]
    async fn a_session_that_mounts_nothing_registers_nothing() {
        let plane = StubDataPlane::default();
        let db = DbHandle::open_in_memory().unwrap();
        let mounts = SessionMounts::default();
        mounts
            .register(&plane, &db, NET, session(""))
            .await
            .unwrap();
        assert!(plane.session_calls().is_empty());

        allocated(&db);
        mounts
            .remove_session_network(
                &crate::system::unavailable::UnavailableContainerRuntime,
                &plane,
                &db,
                NET,
            )
            .await;
        assert!(plane.session_calls().is_empty());
        assert!(!prefix_held(&db), "its prefix is freed");
    }

    // spec: INFRA#mount-endpoint-addressing
    #[tokio::test]
    async fn a_failed_registration_leaves_no_session_behind() {
        let plane = StubDataPlane::default();
        let db = DbHandle::open_in_memory().unwrap();
        let mounts = SessionMounts::default();
        plane.refuse_sessions(true);
        assert!(
            mounts
                .register(&plane, &db, NET, session(".mount(db.port(5432))"))
                .await
                .is_err()
        );
        assert!(plane.session_mounts().is_empty());

        plane.refuse_sessions(false);
        mounts
            .follow(&plane, &db, backends(&["fd5e:ed11:9000:4001::2"]))
            .await;
        assert!(plane.session_mounts().is_empty(), "nothing to rebuild");
        assert_eq!(mounts.failure(), None);
    }

    // spec: INFRA#mount-endpoint-addressing
    // spec: INFRA#pod-networks
    #[tokio::test]
    async fn an_ended_sessions_prefix_is_held_until_its_translations_are_removed() {
        let plane = StubDataPlane::default();
        let db = DbHandle::open_in_memory().unwrap();
        let mounts = SessionMounts::default();
        mounts
            .follow(&plane, &db, backends(&["fd5e:ed11:9000:4001::2"]))
            .await;
        mounts
            .register(&plane, &db, NET, session(".mount(db.port(5432))"))
            .await
            .unwrap();
        allocated(&db);

        plane.refuse_sessions(true);
        mounts
            .remove_session_network(
                &crate::system::unavailable::UnavailableContainerRuntime,
                &plane,
                &db,
                NET,
            )
            .await;
        assert!(
            prefix_held(&db),
            "stale translations would match its prefix"
        );
        assert!(mounts.failure().is_some());

        plane.refuse_sessions(false);
        mounts
            .follow(&plane, &db, backends(&["fd5e:ed11:9000:4001::2"]))
            .await;
        assert_eq!(plane.session_calls().last(), Some(&SessionCall::Reset));
        assert!(!prefix_held(&db), "freed once nothing matches it");
        assert_eq!(mounts.failure(), None);
    }

    // spec: INFRA#mount-endpoint-addressing
    #[tokio::test]
    async fn a_failed_rebuild_is_reported_and_tried_again_after_the_next_apply() {
        let plane = StubDataPlane::default();
        let db = DbHandle::open_in_memory().unwrap();
        let mounts = SessionMounts::default();
        mounts
            .follow(&plane, &db, backends(&["fd5e:ed11:9000:4001::2"]))
            .await;
        mounts
            .register(&plane, &db, NET, session(".mount(db.port(5432))"))
            .await
            .unwrap();

        plane.refuse_sessions(true);
        mounts
            .follow(&plane, &db, backends(&["fd5e:ed11:9000:4002::2"]))
            .await;
        assert!(mounts.failure().is_some());
        assert_eq!(
            held_backends(&plane),
            [vec![("fd5e:ed11:9000:4001::2".parse().unwrap(), 5432)]],
            "the session keeps what it held"
        );

        plane.refuse_sessions(false);
        mounts
            .follow(&plane, &db, backends(&["fd5e:ed11:9000:4002::2"]))
            .await;
        assert_eq!(
            held_backends(&plane),
            [vec![("fd5e:ed11:9000:4002::2".parse().unwrap(), 5432)]]
        );
        assert_eq!(mounts.failure(), None);
    }
}
