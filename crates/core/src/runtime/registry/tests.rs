use seedling_protocol::names::AppName;

use super::{DbInstanceRegistry, InstanceRegistry};
use crate::defs::resource::ResourceKind;
use crate::runtime::db::DbHandle;
use crate::runtime::history;

fn app(s: &str) -> AppName {
    AppName::new(s).unwrap()
}

// r[verify autonomous.healthcheck-replace]
// When a deployment has more existing instances than the requested count, the
// keep set must prefer instances whose lifecycle is Ready (currently healthy)
// over those that aren't. This is what makes the replace flow converge:
// the unhealthy original lands in `excess` once a healthy replacement comes up.
#[test]
fn ensure_scaled_group_prefers_healthy_in_keep() {
    let db = DbHandle::open_in_memory().unwrap();
    let reg = DbInstanceRegistry::new(db.clone());

    // Initial provision: two scaled instances created in order.
    let group = reg
        .ensure_scaled_group(&app("myapp"), ResourceKind::Deployment, Some("web"), 2)
        .unwrap();
    assert_eq!(group.keep.len(), 2);
    assert!(group.excess.is_empty());

    let older = group.keep[0].clone();
    let younger = group.keep[1].clone();

    // The older instance is unhealthy (Running but no health_check_pass);
    // the younger one is Ready.
    db.call({
        let older = older.clone();
        let younger = younger.clone();
        move |db| {
            history::insert_observation(db, &older, "container_running", &serde_json::json!({}))
                .unwrap();
            history::insert_observation(db, &younger, "container_running", &serde_json::json!({}))
                .unwrap();
            history::insert_observation(db, &younger, "health_check_pass", &serde_json::json!({}))
                .unwrap();
        }
    });

    // Asking for count=1 must prefer the Ready instance even though it was
    // created later than the unhealthy one.
    let group = reg
        .ensure_scaled_group(&app("myapp"), ResourceKind::Deployment, Some("web"), 1)
        .unwrap();
    assert_eq!(group.keep.len(), 1);
    assert_eq!(
        group.keep[0].id, younger.id,
        "Ready instance must be in keep"
    );
    assert_eq!(group.excess.len(), 1);
    assert_eq!(
        group.excess[0].id, older.id,
        "unhealthy instance must be excess"
    );
}

// r[verify autonomous.healthcheck-replace.guard]
// When all instances are equally not-Ready (e.g. both unhealthy, or both still
// in start-period), the keep set must fall back to creation order. This is
// what keeps the original instance serving in degraded mode after the
// replace-loop guard trips: the failed replacement is the younger one and
// goes to excess.
#[test]
fn ensure_scaled_group_keeps_oldest_when_all_equally_unhealthy() {
    let db = DbHandle::open_in_memory().unwrap();
    let reg = DbInstanceRegistry::new(db.clone());

    let group = reg
        .ensure_scaled_group(&app("myapp"), ResourceKind::Deployment, Some("web"), 2)
        .unwrap();
    let older = group.keep[0].clone();
    let younger = group.keep[1].clone();

    db.call({
        let older = older.clone();
        let younger = younger.clone();
        move |db| {
            // Both Running but neither Ready.
            history::insert_observation(db, &older, "container_running", &serde_json::json!({}))
                .unwrap();
            history::insert_observation(db, &younger, "container_running", &serde_json::json!({}))
                .unwrap();
        }
    });

    let group = reg
        .ensure_scaled_group(&app("myapp"), ResourceKind::Deployment, Some("web"), 1)
        .unwrap();
    assert_eq!(group.keep.len(), 1);
    assert_eq!(
        group.keep[0].id, older.id,
        "with no preference by health, oldest wins"
    );
    assert_eq!(group.excess.len(), 1);
    assert_eq!(group.excess[0].id, younger.id);
}
