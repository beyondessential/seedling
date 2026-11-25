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
        Err(BundleError::Invalid { message, .. }) => message,
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

// spec: BND#script-formation
// spec: BND#metadata-file
#[test]
fn a_lone_script_is_a_one_file_bundle() {
    let b = Bundle::from_script("let x = 1;\n").unwrap();
    assert_eq!(b.files().keys().collect::<Vec<_>>(), vec!["app.seed.rhai"]);
    let script = b.script().unwrap();
    assert_eq!(script.text(), "let x = 1;\n");
    assert_eq!(script.files().collect::<Vec<_>>(), vec!["app.seed.rhai"]);
}

// spec: BND#script-formation
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

// spec: BND#script-formation
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

// spec: DEFN#bundle-rules
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

// spec: DEFN#bundle-rules
#[test]
fn oversize_bundles_are_rejected() {
    let big = vec![b'x'; BUNDLE_SIZE_LIMIT];
    let m = invalid_message(Bundle::from_files(vec![
        ("app.seed.rhai".to_owned(), b"1".to_vec()),
        ("big.bin".to_owned(), big),
    ]));
    assert!(m.contains("size limit"), "{m}");
}

// spec: DEFN#bundle-rules
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

// spec: DEFN#seedling-version-support
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

// spec: DEFN#content-hash-computation
// spec: GEN#bundle-storage
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

// spec: DEFN#bundle-rules
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

// spec: BND#script-formation
#[test]
fn an_empty_script_file_does_not_shift_the_line_table() {
    let b = Bundle::from_files(files(&[
        (
            "seedling.toml",
            b"script = [\"empty.rhai\", \"a.rhai\", \"b.rhai\"]",
        ),
        ("empty.rhai", b""),
        ("a.rhai", b"let a = 1;\n"),
        ("b.rhai", b"let c = 3;\nthrow \"bad\";\n"),
    ]))
    .unwrap();
    let script = b.script().unwrap();
    assert_eq!(script.text(), "let a = 1;\nlet c = 3;\nthrow \"bad\";\n");
    assert_eq!(
        script.remap_error("Runtime error: bad (line 3, position 1)"),
        "Runtime error: bad (b.rhai line 2, position 1)"
    );
    assert_eq!(
        script.remap_error("at (line 1, position 3)"),
        "at (a.rhai line 1, position 3)"
    );
}

// spec: DEFN#bundle-rules
#[test]
fn bundles_holding_too_many_files_are_rejected() {
    let paths: Vec<(String, Vec<u8>)> = (0..=BUNDLE_FILE_LIMIT)
        .map(|n| (format!("f{n}"), Vec::new()))
        .collect();
    let m = invalid_message(Bundle::from_files(paths.clone()));
    assert!(
        m.contains(&format!("{BUNDLE_FILE_LIMIT}-file limit")),
        "{m}"
    );

    let data = tar_gz(|b| {
        for (path, _) in &paths {
            add_file(b, path, b"");
        }
    });
    let m = invalid_message(Bundle::from_tar_gz(&data));
    assert!(
        m.contains(&format!("{BUNDLE_FILE_LIMIT}-file limit")),
        "{m}"
    );

    // A stored bundle from before the limit still loads.
    let stored = Bundle::from_stored(paths).unwrap();
    assert_eq!(stored.files().len(), BUNDLE_FILE_LIMIT + 1);
}

// spec: DEFN#bundle-rules
#[test]
fn a_file_and_a_folder_of_the_same_name_conflict() {
    for pair in [
        [("a/b", &b"x"[..]), ("a", &b"y"[..])],
        [("a", &b"y"[..]), ("a/b", &b"x"[..])],
        [("a/b/c", &b"x"[..]), ("a/b", &b"y"[..])],
    ] {
        let m = invalid_message(Bundle::from_files(files(&pair)));
        assert!(m.contains("one is inside the other"), "{m}");
    }
    // Neighbours that merely share a prefix do not conflict.
    Bundle::from_files(files(&[("a", b"x"), ("a!b", b"y"), ("ab", b"z")])).unwrap();
    Bundle::from_files(files(&[("a!b", b"y"), ("a/b", b"x")])).unwrap();
}

// spec: DEFN#provenance-reporting
#[test]
fn the_declarations_are_readable_without_rebuilding_the_bundle() {
    let meta: &[u8] = b"seedling = \">=0.12\"\n[metadata]\nversion = \"2.71.4\"";
    let cases: &[&[(&str, &[u8])]] = &[
        &[("app.seed.rhai", b"x"), ("seedling.toml", meta)],
        // A path sorting after the metadata file, and one before it.
        &[
            ("aaa.txt", b"a"),
            ("app.seed.rhai", b"x"),
            ("seedling.toml", meta),
            ("zzz.txt", b"z"),
        ],
    ];
    for files_in in cases {
        let b = Bundle::from_files(files(files_in)).unwrap();
        let read = Bundle::declared_of_canonical(&b.canonical_bytes()).unwrap();
        assert_eq!(read.seedling_versions.as_ref(), b.seedling_versions());
        assert_eq!(&read.metadata, b.metadata().unwrap());
        assert_eq!(read.metadata["version"], "2.71.4");
    }
    let none = Bundle::from_files(files(&[("app.seed.rhai", b"x"), ("zzz.txt", b"z")])).unwrap();
    assert_eq!(
        Bundle::declared_of_canonical(&none.canonical_bytes()).unwrap(),
        Declared::default()
    );
    assert!(Bundle::declared_of_canonical(b"not a bundle").is_err());
}

// spec: BND#metadata-table
#[test]
fn a_bundle_without_a_metadata_table_has_an_empty_one() {
    let bare = Bundle::from_script("x").unwrap();
    assert!(bare.metadata().unwrap().is_empty());
    let no_field = Bundle::from_files(files(&[
        ("seedling.toml", b"seedling = \">=0.12\""),
        ("app.seed.rhai", b"x"),
    ]))
    .unwrap();
    assert!(no_field.metadata().unwrap().is_empty());
}

// spec: BND#metadata-table
#[test]
fn metadata_values_convert_by_type() {
    let b = Bundle::from_files(files(&[
        (
            "seedling.toml",
            br#"
[metadata]
string = "2.71.4"
integer = -7
float = 1.5
whole_float = 2.0
boolean = true
offset_datetime = 1979-05-27 07:32:00.5-07:00
local_datetime = 1979-05-27T07:32:00
local_date = 1979-05-27
local_time = 07:32:00
array = [1, "two", [3.0]]
table = { nested = { deep = false } }
"#,
        ),
        ("app.seed.rhai", b"x"),
    ]))
    .unwrap();
    let expected = serde_json::json!({
        "string": "2.71.4",
        "integer": -7,
        "float": 1.5,
        "whole_float": 2.0,
        "boolean": true,
        "offset_datetime": "1979-05-27T07:32:00.5-07:00",
        "local_datetime": "1979-05-27T07:32:00",
        "local_date": "1979-05-27",
        "local_time": "07:32:00",
        "array": [1, "two", [3.0]],
        "table": { "nested": { "deep": false } },
    });
    assert_eq!(Value::Object(b.metadata().unwrap().clone()), expected);
    assert!(b.metadata().unwrap()["whole_float"].is_f64());
    assert!(b.metadata().unwrap()["integer"].is_i64());
}

// spec: DEFN#bundle-rules
// spec: BND#metadata-table
#[test]
fn unrepresentable_metadata_is_refused_by_where_it_sits() {
    let cases: &[(&[u8], &str)] = &[
        (b"metadata = 1", "`metadata` must be a table, got integer"),
        (
            b"metadata = \"1.2.3\"",
            "`metadata` must be a table, got string",
        ),
        (b"[metadata]\nratio = inf", "`metadata.ratio`"),
        (b"[metadata]\nratio = -inf", "`metadata.ratio`"),
        (
            b"[metadata.limits]\nlist = [1.0, nan]",
            "`metadata.limits.list[1]`",
        ),
    ];
    for (meta, needle) in cases {
        let b = Bundle::from_files(files(&[("seedling.toml", meta), ("app.seed.rhai", b"")]))
            .expect("the strict reading of metadata does not stop a bundle being held");
        let m = match b.metadata() {
            Err(BundleError::Invalid { message, .. }) => message,
            other => panic!("expected invalid metadata, got {other:?}"),
        };
        assert!(m.contains(needle), "{needle}: {m}");
        assert!(b.script().is_err());
    }
}

// spec: BND#metadata-table
// spec: DEFN#provenance-reporting
#[test]
fn a_readable_table_survives_the_rest_of_the_metadata_breaking_the_rules() {
    for meta in [
        &b"future_field = true\n[metadata]\nversion = \"1.0\""[..],
        b"script = [\"missing.rhai\"]\n[metadata]\nversion = \"1.0\"",
    ] {
        let b =
            Bundle::from_files(files(&[("seedling.toml", meta), ("app.seed.rhai", b"")])).unwrap();
        assert!(b.script().is_err());
        assert_eq!(b.metadata().unwrap()["version"], "1.0");
    }
}

// spec: DEFN#seedling-version-support
#[test]
fn an_unsupported_bundle_with_unreadable_metadata_is_reported_unsupported() {
    let b = Bundle::from_files(files(&[
        ("seedling.toml", b"seedling = \">=99\"\nmetadata = 1"),
        ("app.seed.rhai", b""),
    ]))
    .expect("an unsupported bundle is still holdable");
    assert!(matches!(
        b.check_installable(&Version::new(0, 12, 0)),
        Err(BundleError::Unsupported { .. })
    ));
}

fn packable() -> Vec<(String, Vec<u8>)> {
    let deep = format!("{}/page.html", ["nested"; 20].join("/"));
    files(&[
        ("app.seed.rhai", b"let x = 1;"),
        ("pages/error.html", b"<p>oops</p>"),
        ("pages/a.txt", b""),
        ("a.txt", b"top"),
        (&deep, b"deep"),
    ])
}

// spec: CLI#definition-packing
#[test]
fn a_packed_bundle_reads_back_as_the_same_bundle() {
    let bundle = Bundle::from_files(packable()).unwrap();
    let read = Bundle::from_tar_gz(&bundle.to_tar_gz()).unwrap();
    assert_eq!(read, bundle);
    assert_eq!(read.hash(), bundle.hash());
}

// spec: CLI#definition-packing
#[test]
fn packing_the_same_bundle_gives_the_same_bytes() {
    let forwards = Bundle::from_files(packable()).unwrap().to_tar_gz();
    let backwards = Bundle::from_files(packable().into_iter().rev())
        .unwrap()
        .to_tar_gz();
    assert_eq!(forwards, backwards);
}

// spec: CLI#definition-packing
#[test]
fn a_packed_bundle_holds_only_its_files_with_fixed_headers() {
    let data = Bundle::from_files(packable()).unwrap().to_tar_gz();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(data.as_slice()));
    let mut seen: Vec<String> = Vec::new();
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        let header = entry.header();
        assert_eq!(header.entry_type(), tar::EntryType::Regular);
        assert_eq!(header.mtime().unwrap(), 0);
        assert_eq!(header.uid().unwrap(), 0);
        seen.push(entry.path().unwrap().to_string_lossy().into_owned());
    }
    let mut sorted = seen.clone();
    sorted.sort();
    assert_eq!(seen, sorted);
    assert_eq!(seen.len(), packable().len());
}

// spec: CLI#definition-packing
// spec: DEFN#bundle-rules
#[test]
fn a_full_bundle_in_folders_reads_back_from_its_packed_layer() {
    let full: Vec<(String, Vec<u8>)> = (0..BUNDLE_FILE_LIMIT)
        .map(|i| (format!("d{}/e{}/f{i}", i % 64, i % 7), Vec::new()))
        .collect();
    let bundle = Bundle::from_files(full).unwrap();
    assert_eq!(Bundle::from_tar_gz(&bundle.to_tar_gz()).unwrap(), bundle);
}

// spec: DEFN#bundle-rules
#[test]
fn an_archive_with_directory_entries_and_the_full_file_count_is_accepted() {
    let data = tar_gz(|b| {
        for d in 0..16 {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(0o755);
            b.append_data(&mut header, format!("d{d}/"), &[][..])
                .unwrap();
        }
        for i in 0..BUNDLE_FILE_LIMIT {
            add_file(b, &format!("d{}/f{i}", i % 16), b"");
        }
    });
    assert_eq!(
        Bundle::from_tar_gz(&data).unwrap().files().len(),
        BUNDLE_FILE_LIMIT
    );
    let over = tar_gz(|b| {
        for i in 0..=BUNDLE_FILE_LIMIT {
            add_file(b, &format!("f{i}"), b"");
        }
    });
    assert!(invalid_message(Bundle::from_tar_gz(&over)).contains("file limit"));
    let hollow = tar_gz(|b| {
        for d in 0..=BUNDLE_FILE_LIMIT {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(0o755);
            b.append_data(&mut header, format!("d{d}/"), &[][..])
                .unwrap();
        }
    });
    assert!(invalid_message(Bundle::from_tar_gz(&hollow)).contains("not files"));
}

fn with_include(meta: &[u8]) -> Result<Bundle, BundleError> {
    Bundle::from_files(files(&[
        ("seedling.toml", meta),
        ("app.seed.rhai", b""),
        ("lib/db.seed.rhai", b""),
        ("pages/error.html", b""),
    ]))
    .and_then(|b| b.script().cloned().map(|_| b))
}

// spec: BND#included-files
#[test]
fn included_files_and_directories_present_in_the_bundle_are_accepted() {
    with_include(
        b"script = [\"lib/db.seed.rhai\", \"app.seed.rhai\"]\n\
          [include]\n\"lib/db.seed.rhai\" = \"../common/db.seed.rhai\"\n\"./pages/\" = \"../pages\"",
    )
    .unwrap();
}

// spec: BND#included-files
// spec: DEFN#bundle-rules
#[test]
fn an_include_table_breaking_the_rules_is_refused_naming_the_key() {
    let cases: &[(&[u8], &str)] = &[
        (b"include = [\"lib\"]", "table of strings"),
        (b"[include]\n\"lib\" = 1", "\"lib\" must be a string"),
        (
            b"[include]\n\"lib\" = \"\"",
            "\"lib\" has an empty source path",
        ),
        (b"[include]\n\"../lib\" = \"x\"", "\"../lib\" is invalid"),
        (b"[include]\n\"/lib\" = \"x\"", "\"/lib\" is invalid"),
        (b"[include]\n\".\" = \"x\"", "\".\" names the bundle root"),
        (
            b"[include]\n\"seedling.toml\" = \"x\"",
            "names seedling.toml",
        ),
        (
            b"[include]\n\"lib\" = \"x\"\n\"./lib\" = \"y\"",
            "as another key does",
        ),
        (
            b"[include]\n\"lib\" = \"x\"\n\"lib/db.seed.rhai\" = \"y\"",
            "overlaps the key \"lib\"",
        ),
        (
            b"[include]\n\"missing.rhai\" = \"x\"",
            "\"missing.rhai\" holds no file",
        ),
        (b"[include]\n\"li\" = \"x\"", "\"li\" holds no file"),
    ];
    for (meta, needle) in cases {
        let m = invalid_message(with_include(meta));
        assert!(m.contains(needle), "{needle}: {m}");
    }
}

// spec: BND#included-files
// spec: DEFN#seedling-version-support
#[test]
fn an_unsupported_bundle_with_a_malformed_include_is_reported_unsupported() {
    let held = Bundle::from_files(files(&[
        ("seedling.toml", b"seedling = \">=99\"\ninclude = 1"),
        ("app.seed.rhai", b""),
    ]))
    .unwrap();
    assert!(matches!(
        held.check_installable(&semver::Version::new(0, 12, 0)),
        Err(BundleError::Unsupported { .. })
    ));
}

// spec: BND#included-files
#[test]
fn the_include_table_reads_as_normalised_keys_and_their_sources() {
    let table = included(b"seedling = \">=99\"\nfuture = 1\n[include]\n\"./lib/\" = \"../common\"")
        .unwrap();
    assert_eq!(
        table,
        BTreeMap::from([("lib".to_owned(), "../common".to_owned())])
    );
    assert!(included(b"").unwrap().is_empty());
    assert!(included(b"[include]\n\"lib\" = 1").is_err());
}

fn place_of(r: Result<&Script, BundleError>) -> Option<Place> {
    match r {
        Err(BundleError::Invalid { place, .. }) => place,
        other => panic!("expected an invalid bundle, got {other:?}"),
    }
}

// spec: CLI#check-results
#[test]
fn a_metadata_file_that_does_not_parse_lies_at_its_line() {
    let r = Bundle::from_files(files(&[
        ("app.seed.rhai", b""),
        (METADATA_FILE, b"script = [\"app.seed.rhai\"]\n\nbroken =\n"),
    ]));
    match r {
        Err(BundleError::Invalid { place, .. }) => {
            assert_eq!(place, Some(Place::MetadataFile { line: Some(3) }));
        }
        other => panic!("expected an invalid bundle, got {other:?}"),
    }
}

// spec: CLI#check-results
#[test]
fn a_broken_field_lies_in_the_metadata_file_and_a_broken_file_at_its_path() {
    let b = Bundle::from_files(files(&[
        ("app.seed.rhai", b""),
        (METADATA_FILE, b"script = [\"missing.rhai\"]\n"),
    ]))
    .unwrap();
    assert_eq!(
        place_of(b.script()),
        Some(Place::MetadataFile { line: None })
    );
    let b = Bundle::from_files(files(&[("app.seed.rhai", b"\xff")])).unwrap();
    assert_eq!(
        place_of(b.script()),
        Some(Place::Path("app.seed.rhai".to_owned()))
    );
}

// spec: CLI#definition-checks
#[test]
fn the_requirement_is_read_without_the_other_fields() {
    let req = requirement(b"seedling = \">=999\"\nunknown = 1\n").unwrap();
    assert_eq!(req.map(|r| r.to_string()), Some(">=999".to_owned()));
    assert_eq!(requirement(b"unknown = 1\n").unwrap(), None);
}
