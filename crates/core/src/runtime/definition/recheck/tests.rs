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
