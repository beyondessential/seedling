use std::sync::Arc;

use super::*;
use crate::defs::resource::Resource;

fn volume_writes(app: &defs::app::App, name: &str) -> Vec<(String, Vec<u8>)> {
    let def = app.def.load();
    def.resources
        .values()
        .find_map(|r| match r {
            Resource::Volume(v) if v.name.as_deref().map(|n| n.as_str()) == Some(name) => Some(
                v.def
                    .lock()
                    .writes
                    .iter()
                    .map(|(p, c)| (p.clone(), c.to_vec()))
                    .collect(),
            ),
            _ => None,
        })
        .expect("volume exists")
}

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x00, 0xff];

// l[verify volume.write-dir]
// l[verify app.dir]
#[test]
fn write_dir_copies_a_folder_byte_for_byte() {
    let (_, _, app, _) = run_test_script_in(
        r#"app.volume("pages").write_dir("/errors", app.dir("pages"));"#,
        &[
            ("pages/500.html", b"<h1>oops</h1>"),
            ("pages/img/logo.png", PNG),
            ("other.txt", b"not copied"),
        ],
    )
    .unwrap();
    assert_eq!(
        volume_writes(&app, "pages"),
        vec![
            ("/errors/500.html".to_owned(), b"<h1>oops</h1>".to_vec()),
            ("/errors/img/logo.png".to_owned(), PNG.to_vec()),
        ]
    );
}

// l[verify volume.write-dir]
#[test]
fn write_dir_may_target_the_volume_root() {
    let (_, _, app, _) = run_test_script_in(
        r#"app.volume("pages").write_dir("/", app.dir("pages"));"#,
        &[("pages/500.html", b"x")],
    )
    .unwrap();
    assert_eq!(
        volume_writes(&app, "pages"),
        vec![("/500.html".to_owned(), b"x".to_vec())]
    );
    let Err(err) = run_test_script_in(
        r#"app.volume("pages").write_dir("/../x", app.dir("pages"));"#,
        &[("pages/500.html", b"x")],
    ) else {
        panic!("escaping the volume root throws");
    };
    assert!(err.to_string().contains("escape"), "{err}");
}

// l[verify volume.write]
// l[verify app.file]
// l[verify file.type]
#[test]
fn volume_write_with_a_file_writes_its_bytes() {
    let (_, _, app, _) = run_test_script_in(
        r#"app.volume("img").write("/logo.png", app.file("assets/./logo.png"));"#,
        &[("assets/logo.png", PNG)],
    )
    .unwrap();
    assert_eq!(
        volume_writes(&app, "img"),
        vec![("/logo.png".to_owned(), PNG.to_vec())]
    );
}

// l[verify file.text]
#[test]
fn file_text_reads_utf8_and_throws_otherwise() {
    run_test_script_in(
        r#"if app.file("hello.txt").text() != "héllo" { throw "wrong text"; }"#,
        &[("hello.txt", "héllo".as_bytes())],
    )
    .unwrap();
    let Err(err) = run_test_script_in(r#"app.file("logo.png").text();"#, &[("logo.png", PNG)])
    else {
        panic!("invalid UTF-8 throws");
    };
    assert!(err.to_string().contains("not valid UTF-8"), "{err}");
}

// l[verify app.file]
// l[verify app.dir]
#[test]
fn bundle_paths_are_confined_and_must_exist() {
    for (script, needle) in [
        (r#"app.file("/etc/passwd");"#, "absolute"),
        (r#"app.file("../outside");"#, "escapes"),
        (r#"app.file("missing.txt");"#, "no file"),
        (r#"app.dir("empty");"#, "no files beneath"),
    ] {
        let err = run_test_script_in(script, &[("present.txt", b"x")])
            .err()
            .unwrap_or_else(|| panic!("{script} must throw"));
        assert!(err.to_string().contains(needle), "{script}: {err}");
    }
}

// l[verify app.bundle.context]
#[test]
fn old_reads_the_previous_generations_bundle() {
    use crate::runtime::definition::{Bundle, Source};
    use crate::runtime::{db::Db, generations};
    let db = Db::open_in_memory().unwrap();
    let name = seedling_protocol::names::AppName::new("web").unwrap();
    db.conn
        .execute(
            "INSERT INTO registered_apps (name, installed, uninstalling, current_generation) VALUES ('web', 0, 0, 0)",
            [],
        )
        .unwrap();
    let bundle = |page: &[u8]| {
        Bundle::from_files([
            (
                "app.seed.rhai".to_owned(),
                b"let p = app.file(\"page.html\");".to_vec(),
            ),
            ("page.html".to_owned(), page.to_vec()),
        ])
        .unwrap()
    };
    generations::bump_register(&db, &name, &bundle(b"old"), &Source::unknown_push()).unwrap();
    let g =
        generations::bump_script_update(&db, &name, &bundle(b"new"), &Source::unknown_push(), None)
            .unwrap();
    let cipher = crate::runtime::secrets::Cipher::for_tests();
    let limits = crate::ScriptLimits::default();
    let old = generations::reconstruct_app_def(&db, &name, g - 1, &limits, &cipher).unwrap();
    let new = generations::reconstruct_app_def(&db, &name, g, &limits, &cipher).unwrap();
    assert_eq!(old.bundle.file("page.html").unwrap().as_ref(), b"old");
    assert_eq!(new.bundle.file("page.html").unwrap().as_ref(), b"new");
}

// l[verify param.validate.constraints]
#[test]
fn validate_is_top_level_only_and_once_per_param() {
    let err = run_test_script_err(
        r#"
        let p = app.param("mode");
        p.validate(|v, all| {});
        p.validate(|v, all| {});
        "#,
    );
    assert!(err.to_string().contains("already attached"), "{err}");

    let (engine, mut scope, app, _) = run_test_script(r#"app.param("mode");"#);
    let _guard = crate::runtime::barrier::runtime::ActionClosureGuard::new(
        Arc::clone(&app.def),
        "op".to_owned(),
        Default::default(),
    );
    let err = engine
        .run_with_scope(&mut scope, r#"app.param("mode").validate(|v, all| {});"#)
        .expect_err("validate inside an action closure throws");
    assert!(err.to_string().contains("action closure"), "{err}");
}

// l[verify rt.write]
#[test]
fn rt_write_with_a_file_writes_its_bytes() {
    use crate::runtime::barrier::replay::{InMemoryActionLog, OperationResult};
    let writer = Arc::new(super::volume::RecordingVolumeWriter::default());
    let log = InMemoryActionLog::new();
    let result = super::volume::run_action_with_writer_in(
        r#"
        let img = app.volume("img");
        app.on_action("seed", |rt, _param| {
            rt.write(img, "/logo.png", app.file("logo.png"));
        });
        "#,
        &[("logo.png", PNG)],
        "seed",
        Arc::clone(&writer),
        &log,
    );
    assert!(matches!(result, OperationResult::Completed), "{result:?}");
    let writes = writer.writes.lock();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].contents, PNG);
}
