use std::io::Write;

use super::*;

fn files(entries: &[(&str, &[u8])]) -> Vec<(String, Vec<u8>)> {
    entries
        .iter()
        .map(|(p, c)| ((*p).to_owned(), c.to_vec()))
        .collect()
}

fn invalid_message(r: Result<Bundle, BundleError>) -> String {
    match r {
        Err(BundleError::Invalid(m)) => m,
        other => panic!("expected an invalid bundle, got {other:?}"),
    }
}

fn tar_gz(build: impl FnOnce(&mut tar::Builder<Vec<u8>>)) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    build(&mut builder);
    let tar = builder.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap()
}

fn add_file(b: &mut tar::Builder<Vec<u8>>, path: &str, contents: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::Regular);
    b.append_data(&mut header, path, contents).unwrap();
}

// l[verify bsl.bundle]
// l[verify bsl.bundle.metadata]
#[test]
fn a_lone_script_is_a_one_file_bundle() {
    let b = Bundle::from_script("let x = 1;\n").unwrap();
    assert_eq!(b.files().keys().collect::<Vec<_>>(), vec!["app.seed.rhai"]);
    let script = b.script().unwrap();
    assert_eq!(script.text(), "let x = 1;\n");
    assert_eq!(script.files().collect::<Vec<_>>(), vec!["app.seed.rhai"]);
}

// l[verify bsl.bundle]
#[test]
fn listed_scripts_concatenate_in_order() {
    let b = Bundle::from_files(files(&[
        (
            "seedling.toml",
            b"script = [\"lib.rhai\", \"app.seed.rhai\"]",
        ),
        ("lib.rhai", b"fn helper() { 1 }"),
        ("app.seed.rhai", b"let y = helper();\n"),
        ("pages/500.html", b"<h1>oops</h1>"),
    ]))
    .unwrap();
    let script = b.script().unwrap();
    assert_eq!(script.text(), "fn helper() { 1 }\nlet y = helper();\n");
    assert_eq!(
        script.files().collect::<Vec<_>>(),
        vec!["lib.rhai", "app.seed.rhai"]
    );
}

// l[verify bsl.bundle.script-errors]
#[test]
fn error_positions_name_the_file_and_its_own_line() {
    let b = Bundle::from_files(files(&[
        ("seedling.toml", b"script = [\"a.rhai\", \"b.rhai\"]"),
        ("a.rhai", b"let a = 1;\nlet b = 2;\n"),
        ("b.rhai", b"let c = 3;\nthrow \"bad\";\n"),
    ]))
    .unwrap();
    let script = b.script().unwrap();
    assert_eq!(
        script.remap_error("Runtime error: bad (line 4, position 1)"),
        "Runtime error: bad (b.rhai line 2, position 1)"
    );
    assert_eq!(
        script.remap_error("in call (line 1, position 3) inner (line 3, position 9)"),
        "in call (a.rhai line 1, position 3) inner (b.rhai line 1, position 9)"
    );
    assert_eq!(script.remap_error("line of text"), "line of text");
}

// i[verify definition.bundle.limits]
#[test]
fn paths_escaping_the_root_are_rejected() {
    for bad in ["/etc/passwd", "../x", "a/../../x", "a\0b"] {
        let m = invalid_message(Bundle::from_files(files(&[
            ("app.seed.rhai", b""),
            (bad, b"x"),
        ])));
        assert!(m.contains(&format!("{bad:?}")), "{m}");
    }
    let b = Bundle::from_files(files(&[("./dir/../app.seed.rhai", b"")])).unwrap();
    assert!(b.file("app.seed.rhai").is_some());
}

// i[verify definition.bundle.limits]
#[test]
fn oversize_bundles_are_rejected() {
    let big = vec![b'x'; BUNDLE_SIZE_LIMIT];
    let m = invalid_message(Bundle::from_files(vec![
        ("app.seed.rhai".to_owned(), b"1".to_vec()),
        ("big.bin".to_owned(), big),
    ]));
    assert!(m.contains("size limit"), "{m}");
}

// i[verify definition.bundle.limits]
#[test]
fn metadata_breaking_the_rules_is_reported_by_field() {
    let cases: &[(&[u8], &str)] = &[
        (b"script = []", "lists no files"),
        (b"script = [\"a.rhai\", \"./a.rhai\"]", "twice"),
        (b"script = [\"missing.rhai\"]", "missing"),
        (b"script = [\"bin.rhai\"]", "not valid UTF-8"),
        (b"surprise = 1", "surprise"),
        (b"script = \"a.rhai\"", "script"),
        (b"seedling = 13", "seedling"),
        (b"this is = = not toml", "cannot be parsed"),
    ];
    for (meta, needle) in cases {
        let r = Bundle::from_files(files(&[
            ("seedling.toml", meta),
            ("a.rhai", b""),
            ("bin.rhai", &[0xff, 0xfe]),
        ]))
        .and_then(|b| b.script().cloned().map(|_| b));
        let m = invalid_message(r);
        assert!(m.contains(needle), "{needle}: {m}");
    }
}

// i[verify definition.bundle.seedling-versions]
#[test]
fn an_unsupported_requirement_is_reported_before_unknown_fields() {
    let b = Bundle::from_files(files(&[
        ("seedling.toml", b"seedling = \">=99\"\nfuture_field = true"),
        ("app.seed.rhai", b""),
    ]))
    .expect("an unsupported bundle is still holdable");
    match b.check_installable(&Version::new(0, 12, 0)) {
        Err(BundleError::Unsupported { requirement, .. }) => assert_eq!(requirement, ">=99"),
        other => panic!("expected unsupported, got {other:?}"),
    }
    assert!(b.check_installable(&Version::new(99, 0, 0)).is_err());
    assert!(b.supports(&Version::new(99, 0, 0)));
}

// i[verify definition.content-hash]
// r[verify generation.script-storage]
#[test]
fn hash_depends_only_on_paths_and_contents() {
    let pushed =
        Bundle::from_files(files(&[("app.seed.rhai", b"x"), ("pages/a.html", b"a")])).unwrap();
    let reordered =
        Bundle::from_files(files(&[("./pages/a.html", b"a"), ("app.seed.rhai", b"x")])).unwrap();
    let fetched = Bundle::from_tar_gz(&tar_gz(|b| {
        let mut dir = tar::Header::new_gnu();
        dir.set_entry_type(tar::EntryType::Directory);
        dir.set_size(0);
        b.append_data(&mut dir, "pages/", std::io::empty()).unwrap();
        add_file(b, "pages/a.html", b"a");
        add_file(b, "app.seed.rhai", b"x");
    }))
    .unwrap();
    assert_eq!(pushed.hash(), reordered.hash());
    assert_eq!(pushed.hash(), fetched.hash());
    assert!(pushed.hash().starts_with("sha256:"));
    let other =
        Bundle::from_files(files(&[("app.seed.rhai", b"y"), ("pages/a.html", b"a")])).unwrap();
    assert_ne!(pushed.hash(), other.hash());
}

// i[verify definition.bundle.limits]
#[test]
fn archives_with_links_are_rejected() {
    let data = tar_gz(|b| {
        add_file(b, "app.seed.rhai", b"x");
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_size(0);
        b.append_link(&mut link, "evil", "/etc/passwd").unwrap();
    });
    let m = invalid_message(Bundle::from_tar_gz(&data));
    assert!(m.contains("evil"), "{m}");
}

#[test]
fn canonical_and_wire_forms_round_trip() {
    let b = Bundle::from_files(files(&[
        ("app.seed.rhai", b"x"),
        ("bin/blob", &[0, 1, 2, 255]),
    ]))
    .unwrap();
    assert_eq!(
        Bundle::from_canonical_bytes(&b.canonical_bytes()).unwrap(),
        b
    );
    assert_eq!(Bundle::from_wire(&b.to_wire()).unwrap(), b);
}
