use super::*;
use crate::defs::resource::ResourceKind;

fn app() -> AppName {
    AppName::new("myapp").unwrap()
}

fn instance() -> ResourceInstance {
    ResourceInstance::new_singleton(app(), ResourceKind::Deployment, "web")
}

fn active(db: &Db) -> Vec<faults::FaultRecord> {
    faults::list_active_faults(db, Some(&app()))
        .expect("list")
        .into_iter()
        .filter(|f| f.kind == "health_check_failed")
        .collect()
}

// spec: FLT#workload-faults
#[test]
fn an_unscheduled_instance_clears_its_health_check_failed_fault() {
    let db = Db::open_in_memory().expect("open");
    let inst = instance();
    apply_health_check_faults(&db, &app(), std::slice::from_ref(&inst), &[]);
    assert_eq!(active(&db).len(), 1);

    apply_health_check_faults(&db, &app(), &[], std::slice::from_ref(&inst));
    assert!(active(&db).is_empty());
}

// spec: FLT#workload-faults
#[test]
fn clearing_is_scoped_to_the_instance_that_was_unscheduled() {
    let db = Db::open_in_memory().expect("open");
    let (a, b) = (
        instance(),
        ResourceInstance::new_singleton(app(), ResourceKind::Deployment, "api"),
    );
    apply_health_check_faults(&db, &app(), &[a.clone(), b.clone()], &[]);
    assert_eq!(active(&db).len(), 2);

    apply_health_check_faults(&db, &app(), &[], &[a]);
    let left = active(&db);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].instance_id.as_deref(), Some(b.id.to_hex().as_str()));
}
