use super::*;
use crate::defs::resource::ResourceKind;

fn app() -> AppName {
    AppName::new("myapp").unwrap()
}

fn kinds(db: &Db) -> Vec<(String, Option<String>)> {
    faults::list_active_faults(db, Some(&app()))
        .expect("list")
        .into_iter()
        .map(|f| (f.kind, f.instance_id))
        .collect()
}

// spec: FLT#workload-faults
#[test]
fn an_unreadable_instance_identity_is_its_own_fault_kind_and_clears_when_it_starts() {
    let db = Db::open_in_memory().expect("open");
    let instance = ResourceInstance::new_singleton(app(), ResourceKind::Deployment, "web");

    Reconciler::file_instance_registry_fault_inner(&db, &app(), &instance);
    assert_eq!(
        kinds(&db),
        vec![(
            "instance_identity_unreadable".to_owned(),
            Some(instance.id.to_hex())
        )]
    );

    Reconciler::clear_instance_registry_fault_inner(&db, &app(), &instance);
    assert!(kinds(&db).is_empty());
}
