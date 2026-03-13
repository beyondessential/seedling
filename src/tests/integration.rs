use super::*;
use crate::run_file;

// l[verify bsl.script]
#[test]
fn draft_script_runs_and_exercises() {
    let (engine, mut scope, app) = setup();
    let ast = run_file(
        &engine,
        &mut scope,
        std::path::PathBuf::from("draft-beset.rhai"),
    )
    .expect("draft script should run");

    let def = app.0.lock();
    assert!(def.params.contains_key("domain"));
    assert!(def.params.contains_key("version"));
    assert!(def.actions.contains_key("start"));
    assert!(def.actions.contains_key("upgrade"));
    assert!(def.actions.contains_key("migrate"));
    assert!(def.shells.contains_key("node"));
    assert!(def.shells.contains_key("db"));
    assert!(def.install.is_some());

    let install = def.install.as_ref().unwrap();
    assert!(install.requirements.contains_key("admin_user_email"));
    assert!(install.requirements.contains_key("admin_user_password"));
    drop(def);

    exercise_actions(&engine, &mut scope, &app, &ast);
}
