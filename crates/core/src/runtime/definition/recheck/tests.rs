use std::sync::atomic::Ordering;

use tokio::sync::Notify;

use super::*;
use crate::runtime::{
    definition::{Bundle, fetch::testing::FakeRegistry},
    faults::list_active_faults,
};

const REPO: &str = "ghcr.io/org/app-def";
const MIN: Duration = Duration::from_secs(60);

fn app(name: &str) -> AppName {
    AppName::new(name).unwrap()
}

fn running() -> Version {
    Version::new(0, 12, 0)
}

fn block<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(f)
}

struct Harness {
    apps: RwLock<AppRegistry>,
    db: DbHandle,
    registry: FakeRegistry,
    rechecker: Rechecker,
    start: Instant,
}

impl Harness {
    fn new() -> Self {
        Self {
            apps: RwLock::new(AppRegistry::new()),
            db: DbHandle::open_in_memory().unwrap(),
            registry: FakeRegistry::default(),
            // Zero interval and no jitter to speak of: due on every pass
            // unless backing off.
            rechecker: Rechecker::new(Duration::ZERO, 10 * MIN, 80 * MIN),
            start: Instant::now(),
        }
    }

    fn publish(&self, tag: &str, body: &str) -> String {
        let digest =
            self.registry
                .push_definition(REPO, &[("app.seed.rhai", body.as_bytes())], None);
        self.registry.tag(REPO, tag, &digest);
        digest
    }

    fn register(&self, name: &str, source: Source) {
        self.apps.write().insert_registered(
            app(name),
            Arc::new(Bundle::from_stored_script("")),
            source,
            Default::default(),
            None,
            Arc::new(Notify::new()),
            1,
        );
    }

    fn fetched(&self, name: &str, reference: &str, digest: &str) {
        self.register(
            name,
            Source::Fetched {
                reference: reference.to_owned(),
                digest: digest.to_owned(),
            },
        );
    }

    fn pass_at(&mut self, offset: Duration) {
        let now = self.start + offset;
        block(
            self.rechecker
                .pass(&self.apps, &self.db, &self.registry, &running(), now),
        );
    }

    fn moved_faults(&self, name: &str) -> Vec<String> {
        let a = app(name);
        self.db
            .call(move |db| list_active_faults(db, Some(&a)).unwrap())
            .into_iter()
            .filter(|f| f.kind == faults::SOURCE_MOVED)
            .map(|f| f.description)
            .collect()
    }
}

// r[verify fault.definition-source-moved]
// r[verify definition.recheck]
#[test]
fn a_moved_tag_files_the_fault_and_moving_back_clears_it() {
    let mut h = Harness::new();
    let first = h.publish("1", "// one");
    h.fetched("web", &format!("{REPO}:1"), &first);

    h.pass_at(Duration::ZERO);
    assert!(h.moved_faults("web").is_empty());

    let second = h.publish("1", "// two");
    h.pass_at(MIN);
    let faults = h.moved_faults("web");
    assert_eq!(faults.len(), 1);
    assert!(
        faults[0].contains(&format!("{REPO}:1"))
            && faults[0].contains(&first)
            && faults[0].contains(&second),
        "{faults:?}"
    );

    h.registry.tag(REPO, "1", &first);
    h.pass_at(2 * MIN);
    assert!(h.moved_faults("web").is_empty());
}

// r[verify fault.definition-source-moved]
#[test]
fn a_failed_recheck_leaves_the_fault_as_it_was() {
    let mut h = Harness::new();
    let first = h.publish("1", "// one");
    h.fetched("web", &format!("{REPO}:1"), &first);
    h.publish("1", "// two");
    h.pass_at(Duration::ZERO);
    assert_eq!(h.moved_faults("web").len(), 1);

    h.registry.set_unreachable(true);
    h.registry.tag(REPO, "1", &first);
    h.pass_at(MIN);
    assert_eq!(h.moved_faults("web").len(), 1, "a failure must not clear");

    // And the reverse: no fault, a failure files none.
    let mut h = Harness::new();
    let first = h.publish("1", "// one");
    h.fetched("web", &format!("{REPO}:1"), &first);
    h.publish("1", "// two");
    h.registry.set_unreachable(true);
    h.pass_at(Duration::ZERO);
    assert!(h.moved_faults("web").is_empty(), "a failure must not file");
}

// r[verify definition.recheck.backoff]
#[test]
fn failures_back_off_to_a_cap_and_a_success_resets() {
    let mut h = Harness::new();
    let first = h.publish("1", "// one");
    h.fetched("web", &format!("{REPO}:1"), &first);
    h.registry.set_unreachable(true);

    let requests = |h: &Harness| h.registry.requests.load(Ordering::SeqCst);
    h.pass_at(Duration::ZERO);
    assert_eq!(requests(&h), 1);
    // Base delay of 10 minutes: nothing at 5, a retry at 10.
    h.pass_at(5 * MIN);
    assert_eq!(requests(&h), 1);
    h.pass_at(10 * MIN);
    assert_eq!(requests(&h), 2);
    // Now 20 minutes after the second failure.
    h.pass_at(25 * MIN);
    assert_eq!(requests(&h), 2);
    h.pass_at(30 * MIN);
    assert_eq!(requests(&h), 3);
    // However long it has failed, the cap is always enough.
    h.pass_at(30 * MIN + 80 * MIN);
    assert_eq!(requests(&h), 4);

    h.registry.set_unreachable(false);
    h.pass_at(30 * MIN + 160 * MIN);
    assert_eq!(requests(&h), 5);
    // Healthy again: due on the (zero) interval, not the back-off.
    h.pass_at(30 * MIN + 161 * MIN);
    assert_eq!(requests(&h), 6);
}

// r[verify definition.recheck.backoff]
// r[verify definition.recheck]
#[test]
fn one_apps_failure_does_not_delay_another() {
    let mut h = Harness::new();
    let first = h.publish("1", "// one");
    h.fetched("good", &format!("{REPO}:1"), &first);
    // Not on the allowlist: fails without any request.
    h.fetched("bad", "registry.example.com/org/app-def:1", "sha256:0");

    h.pass_at(Duration::ZERO);
    let after_first = h.registry.requests.load(Ordering::SeqCst);
    assert_eq!(after_first, 1, "only the allowed app reached the registry");

    h.publish("1", "// two");
    h.pass_at(MIN);
    assert_eq!(h.moved_faults("good").len(), 1);
}

// r[verify definition.recheck]
#[test]
fn pinned_and_pushed_definitions_are_never_rechecked() {
    let mut h = Harness::new();
    let first = h.publish("1", "// one");
    h.fetched("pinned", &format!("{REPO}@{first}"), &first);
    h.register("pushed", Source::unknown_push());
    h.publish("1", "// two");
    h.pass_at(Duration::ZERO);
    h.pass_at(MIN);
    assert_eq!(h.registry.requests.load(Ordering::SeqCst), 0);
    assert!(h.moved_faults("pinned").is_empty());
    assert!(h.moved_faults("pushed").is_empty());
}

// r[verify definition.recheck]
#[test]
fn a_recheck_reads_only_manifests() {
    let mut h = Harness::new();
    let first = h.publish("1", "// one");
    h.fetched("web", &format!("{REPO}:1"), &first);
    h.publish("1", "// two");
    h.pass_at(Duration::ZERO);
    assert!(h.registry.requests.load(Ordering::SeqCst) > 0);
    assert_eq!(h.registry.blob_requests.load(Ordering::SeqCst), 0);
    // The app itself is untouched.
    let apps = h.apps.read();
    let entry = apps.get("web").unwrap();
    assert!(matches!(&entry.source, Source::Fetched { digest, .. } if *digest == first));
}

// r[verify definition.recheck]
#[test]
fn a_registry_removed_from_the_allowlist_is_not_contacted() {
    let mut h = Harness::new();
    let first = h.publish("1", "// one");
    h.fetched("web", &format!("{REPO}:1"), &first);
    h.db.call(|db| crate::runtime::registries::remove_allowed_registry(db, "ghcr.io"))
        .unwrap();
    h.publish("1", "// two");
    h.pass_at(Duration::ZERO);
    assert_eq!(h.registry.requests.load(Ordering::SeqCst), 0);
    assert!(h.moved_faults("web").is_empty());
}

// r[verify fault.definition-source-moved]
// r[verify fault.lifecycle]
#[test]
fn a_filed_fault_survives_a_restart_and_a_failed_recheck() {
    let mut h = Harness::new();
    let first = h.publish("1", "// one");
    h.fetched("web", &format!("{REPO}:1"), &first);
    h.publish("1", "// two");
    h.pass_at(Duration::ZERO);
    assert_eq!(h.moved_faults("web").len(), 1);

    // A fresh rechecker is what a restarted daemon has: no memory, only the
    // database.
    h.rechecker = Rechecker::new(Duration::ZERO, 10 * MIN, 80 * MIN);
    h.registry.set_unreachable(true);
    h.pass_at(MIN);
    assert_eq!(h.moved_faults("web").len(), 1);
}

// r[verify definition.recheck.cadence]
#[test]
fn rechecks_are_infrequent_and_spread() {
    assert!(RECHECK_INTERVAL >= Duration::from_secs(60 * 60));
    let spreads: std::collections::BTreeSet<u64> = (0..64)
        .map(|_| jitter(RECHECK_INTERVAL).as_secs())
        .collect();
    assert!(
        spreads
            .iter()
            .all(|s| *s <= RECHECK_INTERVAL.as_secs() / 10)
    );
    assert!(spreads.len() > 1, "hosts do not all pick the same moment");
}

/// A registry that takes `delay` to answer, as an unreachable host does
/// before its connect and read timeouts expire.
struct SlowRegistry<'a> {
    inner: &'a FakeRegistry,
    delay: Duration,
}

impl Registry for SlowRegistry<'_> {
    fn manifest<'a>(
        &'a self,
        reference: &'a oci_client::Reference,
    ) -> futures_util::future::BoxFuture<'a, Result<(bytes::Bytes, String), String>> {
        Box::pin(async move {
            tokio::time::sleep(self.delay).await;
            self.inner.manifest(reference).await
        })
    }

    fn blob<'a>(
        &'a self,
        reference: &'a oci_client::Reference,
        digest: &'a str,
        limit: usize,
    ) -> futures_util::future::BoxFuture<'a, Result<bytes::Bytes, String>> {
        self.inner.blob(reference, digest, limit)
    }

    fn tags<'a>(
        &'a self,
        reference: &'a oci_client::Reference,
    ) -> futures_util::future::BoxFuture<'a, Result<Vec<String>, String>> {
        self.inner.tags(reference)
    }
}

// r[verify definition.recheck]
#[test]
fn a_slow_registry_does_not_hold_up_the_other_apps() {
    let mut h = Harness::new();
    let digest = h.publish("1", "// one");
    let apps = 8;
    for i in 0..apps {
        h.fetched(&format!("app{i}"), &format!("{REPO}:1"), &digest);
    }
    let delay = Duration::from_secs(60);
    let slow = SlowRegistry {
        inner: &h.registry,
        delay,
    };
    let start = h.start;
    let rechecker = &mut h.rechecker;
    let taken = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(async {
            let from = tokio::time::Instant::now();
            rechecker
                .pass(&h.apps, &h.db, &slow, &running(), start)
                .await;
            from.elapsed()
        });
    assert!(
        taken < delay * 2,
        "a pass over {apps} apps took {taken:?}, so they were asked one after another"
    );
    assert_eq!(h.registry.requests.load(Ordering::SeqCst), apps);
}
