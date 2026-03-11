use super::*;
use defs::resource::ResourceKind;

// l[verify volume.type]
#[test]
fn volume_named() {
    let app = run_test_script_app(
        r#"
        let v = app.volume("data");
    "#,
    );
    let def = app.0.lock();
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::Volume && &*id.name == "data")
    );
}

// l[verify volume.type]
#[test]
fn volume_anonymous() {
    run_test_script_app(
        r#"
        let v = app.volume();
    "#,
    );
}

// l[verify volume.readonly]
#[test]
fn volume_readonly() {
    let app = run_test_script_app(
        r#"
        let v = app.volume("cfg").readonly();
    "#,
    );
    let def = app.0.lock();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Volume && &*id.name == "cfg")
        .unwrap();
    if let defs::resource::Resource::Volume(vol) = &def.resources[id] {
        assert!(vol.def.lock().read_only);
    } else {
        panic!("expected Volume");
    }
}

// l[verify volume.write]
#[test]
fn volume_write() {
    let app = run_test_script_app(
        r#"
        let v = app.volume("cfg");
        v.write("/app.conf", "key=value");
    "#,
    );
    let def = app.0.lock();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Volume && &*id.name == "cfg")
        .unwrap();
    if let defs::resource::Resource::Volume(vol) = &def.resources[id] {
        let vol_def = vol.def.lock();
        assert_eq!(vol_def.writes.len(), 1);
        assert_eq!(vol_def.writes[0], ("/app.conf".into(), "key=value".into()));
    } else {
        panic!("expected Volume");
    }
}

// l[verify volume.write]
#[test]
fn volume_write_multiple() {
    let app = run_test_script_app(
        r#"
        let v = app.volume("cfg");
        v.write("/a.conf", "aaa");
        v.write("/b.conf", "bbb");
    "#,
    );
    let def = app.0.lock();
    let id = def
        .resources
        .keys()
        .find(|id| id.kind == ResourceKind::Volume && &*id.name == "cfg")
        .unwrap();
    if let defs::resource::Resource::Volume(vol) = &def.resources[id] {
        let vol_def = vol.def.lock();
        assert_eq!(vol_def.writes.len(), 2);
    } else {
        panic!("expected Volume");
    }
}

// l[verify volume.external]
#[test]
fn external_volume_creates_resource() {
    let app = run_test_script_app(
        r#"
        let v = app.external_volume("pg-socket");
    "#,
    );
    let def = app.0.lock();
    assert!(
        def.resources
            .keys()
            .any(|id| id.kind == ResourceKind::ExternalVolume && &*id.name == "pg-socket")
    );
}

// l[verify volume.external]
#[test]
fn external_volume_can_be_mounted() {
    run_test_script_app(
        r#"
        let evol = app.external_volume("shared");
        app.deployment("web")
            .image("nginx")
            .mount("/shared", evol);
    "#,
    );
}
