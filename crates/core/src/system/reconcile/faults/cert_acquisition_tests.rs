use super::*;
use crate::defs::resource::ResourceKind;

fn app() -> AppName {
    AppName::new("myapp").unwrap()
}

fn ingress() -> ResourceInstance {
    ResourceInstance::new_singleton(app(), ResourceKind::Ingress, "web")
}

fn active(db: &Db) -> Vec<faults::FaultRecord> {
    faults::list_active_faults(db, Some(&app()))
        .expect("list")
        .into_iter()
        .filter(|f| f.kind == "cert_acquisition_failed")
        .collect()
}

// spec: FLT#certificate-faults
#[test]
fn the_description_names_each_failing_hostname_and_its_latest_error() {
    let description = describe_cert_acquisition(&[
        (
            "a.example.com".to_owned(),
            AcquisitionError::Recorded("dns record not found".to_owned()),
        ),
        ("b.example.com".to_owned(), AcquisitionError::None),
        ("c.example.com".to_owned(), AcquisitionError::Unreadable),
    ]);
    assert!(
        description.contains("a.example.com (latest acquisition error: dns record not found)"),
        "{description}"
    );
    assert!(description.contains("b.example.com (no acquisition error recorded)"));
    assert!(description.contains("c.example.com (latest acquisition error could not be read)"));
}

// spec: FLT#certificate-faults
#[test]
fn the_fault_is_filed_once_and_its_description_follows_the_failing_hostnames() {
    let db = Db::open_in_memory().expect("open");
    let inst = ingress();

    apply_cert_acquisition_fault(&db, &inst, "first");
    apply_cert_acquisition_fault(&db, &inst, "second");

    let faults = active(&db);
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].description, "second");
    assert_eq!(
        faults[0].instance_id.as_deref(),
        Some(inst.id.to_hex().as_str())
    );
}
