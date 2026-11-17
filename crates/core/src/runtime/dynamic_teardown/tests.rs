use std::sync::Arc;

use parking_lot::Mutex;
use seedling_protocol::names::AppName;

use super::{keep_tearing_down, tear_down};
use crate::{
    defs::resource::ResourceKind,
    runtime::{
        anonymous_volumes::{self, AnonymousVolume},
        db::DbHandle,
        desired::{insert_dynamic_resource, list_dynamic_resources},
        faults,
        identity::ResourceInstance,
        scheduler::Scheduler,
    },
    system::{System, stub::StubContainerRuntime},
};

const TEN_MINUTES: std::time::Duration = std::time::Duration::from_secs(600);

struct Harness {
    driver: Arc<System>,
    containers: Arc<StubContainerRuntime>,
    db: DbHandle,
    _dir: tempfile::TempDir,
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let containers = Arc::new(StubContainerRuntime::new(dir.path().join("vols")));
    let process = Arc::new(crate::system::stub::StubProcessManager::new(Arc::clone(
        &containers,
    )));
    let backends = crate::system::backends::Backends::working(
        Arc::clone(&containers) as Arc<dyn crate::system::ContainerRuntime>,
        process,
    );
    let driver = Arc::new(System {
        container: backends.container(),
        process: backends.process(),
        proxy: Arc::new(crate::system::stub::StubNetworkProxy),
        data_plane: Arc::new(crate::system::stub::StubDataPlane),
        volume_store: crate::system::volume_store::VolumeStore::new(dir.path(), false).unwrap(),
        backends,
    });
    Harness {
        driver,
        containers,
        db: DbHandle::open_in_memory().unwrap(),
        _dir: dir,
    }
}

/// Record an anonymous Job of `op` whose anonymous volume, standing on the
/// host, the container runtime refuses to remove.
fn stuck_job(h: &Harness, op: &str) -> ResourceInstance {
    let job = ResourceInstance::new_anonymous(AppName::new("app").unwrap(), ResourceKind::Job);
    let volume = AnonymousVolume {
        name: "seedling-anon-0123456789abcdef0123456789abcdef-vol-0".to_owned(),
        tmpfs: false,
    };
    let (recorded, op, stored) = (job.clone(), op.to_owned(), volume.clone());
    h.db.call(move |db| {
        insert_dynamic_resource(db, &recorded, &op, None).unwrap();
        anonymous_volumes::record(db, &recorded, &[stored]).unwrap();
    });
    let created = volume.name.clone();
    with_paused_clock(async { h.driver.container.create_volume(&created, false).await }).unwrap();
    h.containers.refuse_volume_removal(true);
    job
}

fn stalled(h: &Harness, op: &str) -> usize {
    let op = op.to_owned();
    h.db.call(move |db| {
        faults::list_active_faults(db, None)
            .unwrap()
            .into_iter()
            .filter(|f| f.kind == faults::OPERATION_CLEANUP_STALLED && f.subject == op)
            .count()
    })
}

fn records(h: &Harness) -> usize {
    h.db.call(|db| list_dynamic_resources(db).unwrap().len())
}

fn with_paused_clock<F: std::future::Future>(body: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(body)
}

// spec: OPS#completion
// spec: FLT#app-faults
// A resource the startup teardown could not remove was left for the next
// start, with nothing trying again and nothing telling an operator.
#[test]
fn a_dynamic_resource_left_by_a_teardown_is_torn_down_again_holding_the_fault_meanwhile() {
    let h = harness();
    stuck_job(&h, "op-1");
    let scheduler = Arc::new(Mutex::new(Scheduler::new()));

    with_paused_clock(async {
        let left = tear_down(&h.driver, &h.db, h.db.call(list_dynamic_resources).unwrap()).await;
        assert_eq!(left.len(), 1, "the first teardown leaves it");

        let keep = keep_tearing_down(
            Arc::clone(&h.driver),
            h.db.clone(),
            Arc::clone(&scheduler),
            Some(left),
        );
        tokio::pin!(keep);
        tokio::select! {
            () = &mut keep => panic!("the teardown gave up while the resource stood"),
            () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
        }
        assert_eq!(stalled(&h, "op-1"), 0, "not within the first minute");
        tokio::select! {
            () = &mut keep => panic!("the teardown gave up while the resource stood"),
            () = tokio::time::sleep(TEN_MINUTES) => {}
        }
        assert_eq!(stalled(&h, "op-1"), 1);
        assert_eq!(records(&h), 1, "the record is kept");

        h.containers.refuse_volume_removal(false);
        tokio::time::timeout(TEN_MINUTES, keep)
            .await
            .expect("the teardown ends once the resource goes");
    });
    assert_eq!(records(&h), 0);
    assert_eq!(
        stalled(&h, "op-1"),
        0,
        "cleared once the last resource went"
    );
}

/// How many `dynamic_resources_unreadable` faults are active.
fn unreadable(h: &Harness) -> usize {
    h.db.call(|db| {
        faults::list_active_faults(db, None)
            .unwrap()
            .into_iter()
            .filter(|f| {
                f.kind == faults::DYNAMIC_RESOURCES_UNREADABLE && f.app.as_str() == "_system"
            })
            .count()
    })
}

// spec: OPS#completion
// spec: SUSP#abandoned-replay
// spec: FLT#host-faults
// Records the startup teardown could not read were left for the next start,
// with nothing telling an operator.
#[test]
fn dynamic_resources_whose_records_could_not_be_read_are_read_again_and_torn_down() {
    let h = harness();
    stuck_job(&h, "op-1");
    h.containers.refuse_volume_removal(false);
    let rename = |from: &'static str, to: &'static str| {
        h.db.call(move |db| {
            db.conn
                .execute(&format!("ALTER TABLE {from} RENAME TO {to}"), [])
                .unwrap();
        });
    };
    rename("dynamic_resources", "hidden_dynamic_resources");
    let scheduler = Arc::new(Mutex::new(Scheduler::new()));

    with_paused_clock(async {
        let keep = keep_tearing_down(
            Arc::clone(&h.driver),
            h.db.clone(),
            Arc::clone(&scheduler),
            None,
        );
        tokio::pin!(keep);
        tokio::select! {
            () = &mut keep => panic!("the teardown ended without reading the records"),
            () = tokio::time::sleep(TEN_MINUTES) => {}
        }
        assert_eq!(unreadable(&h), 1, "held while the records cannot be read");
        rename("hidden_dynamic_resources", "dynamic_resources");
        tokio::time::timeout(TEN_MINUTES, keep)
            .await
            .expect("the teardown ends once the records are read and torn down");
    });
    assert_eq!(records(&h), 0);
    assert_eq!(unreadable(&h), 0, "cleared once the records are read");
}

// spec: OPS#completion
// spec: FLT#app-faults
// The fault of an operation whose last record went before the fault was
// cleared, as across a restart, outlived it.
#[test]
fn a_fault_outliving_its_operations_records_is_cleared() {
    let h = harness();
    let app = AppName::new("app").unwrap();
    h.db.call(move |db| {
        faults::hold_operation_cleanup_stalled(db, &app, "op-gone", "test");
        faults::hold_operation_cleanup_stalled(db, &app, "op-live", "test");
    });
    let job = stuck_job(&h, "op-left");
    h.db.call(move |db| {
        faults::hold_operation_cleanup_stalled(db, &job.app, "op-left", "test");
    });
    let left = h.db.call(list_dynamic_resources).unwrap();

    super::clear_finished_operations(&h.db, &left, &["op-live".to_owned()].into());

    assert_eq!(stalled(&h, "op-gone"), 0);
    assert_eq!(stalled(&h, "op-live"), 1, "a live operation's is left");
    assert_eq!(stalled(&h, "op-left"), 1, "one with records left keeps it");
}
