use super::*;

// l[verify history.type]
// l[verify history.var]
#[test]
fn history_available_in_crash_recovery() {
    exercise(
        r#"
        app.on_crash_recovery(|rt, history| {
            let t = history.type_of();
            if t != "History" { throw "history must be History, got: " + t; }
        });
    "#,
    );
}

// l[verify history.was-upgrading]
#[test]
fn history_was_upgrading_returns_bool() {
    exercise(
        r#"
        app.on_crash_recovery(|rt, history| {
            let was = history.was_upgrading();
            if was.type_of() != "bool" { throw "was_upgrading must return bool"; }
        });
    "#,
    );
}
