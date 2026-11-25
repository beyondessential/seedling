use super::*;

fn write(root: &Path, rel: &str, contents: &[u8]) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

// spec: CLI#definition-sources
#[test]
fn github_folder_urls_split_into_every_ref_and_path() {
    let url =
        GithubUrl::parse("https://github.com/org/repo/tree/release/2.12/apps/tamanu").unwrap();
    assert_eq!((url.owner.as_str(), url.repo.as_str()), ("org", "repo"));
    let splits: Vec<_> = url.splits().collect();
    assert_eq!(
        splits[0],
        ("release".to_owned(), "2.12/apps/tamanu".to_owned())
    );
    assert_eq!(
        splits[1],
        ("release/2.12".to_owned(), "apps/tamanu".to_owned())
    );
    assert!(GithubUrl::parse("https://github.com/org/repo").is_none());
    assert!(GithubUrl::parse("./local/folder").is_none());
}

// spec: CLI#definition-sources
#[test]
fn a_pushed_bundle_carries_its_origin() {
    let def = Definition::Bundle {
        bundle: Box::new(
            Bundle::from_files([("app.seed.rhai".to_owned(), b"app;".to_vec())]).unwrap(),
        ),
        origin: Some(("https://github.com/o/r/tree/main/x".into(), "abc".into())),
    };
    let fields = def.fields(DefinitionKeys::APP);
    assert_eq!(fields["bundle"]["app.seed.rhai"], BASE64.encode(b"app;"));
    assert_eq!(fields["origin"]["revision"], "abc");
    assert!(def.fields(DefinitionKeys::TEMPLATE).contains_key("bundle"));
    assert_eq!(
        Definition::Reference("ghcr.io/o/d:1".into()).fields(DefinitionKeys::APP)["reference"],
        "ghcr.io/o/d:1"
    );
}

// spec: CLI#parameter-changes
#[test]
fn several_parameter_changes_make_one_params_object() {
    let strings = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let changes = param_changes(
        &strings(&["version=v2.12", "empty=", "url=https://x/?a=b"]),
        &strings(&["old", "older"]),
    )
    .unwrap();
    assert_eq!(
        Value::Object(changes),
        json!({
            "version": "v2.12",
            "empty": "",
            "url": "https://x/?a=b",
            "old": null,
            "older": null,
        })
    );
    assert!(param_changes(&[], &[]).unwrap().is_empty());
}

// spec: CLI#parameter-changes
#[test]
fn a_parameter_changed_twice_or_a_set_without_a_value_is_refused() {
    let strings = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert!(param_changes(&strings(&["bare"]), &[]).is_err());
    assert!(param_changes(&strings(&["a=1", "a=2"]), &[]).is_err());
    assert!(param_changes(&[], &strings(&["a", "a"])).is_err());
    let err = param_changes(&strings(&["a=1"]), &strings(&["a"])).unwrap_err();
    assert!(err.contains("\"a\""), "{err}");
}

// spec: CLI#definition-export
#[test]
fn export_reproduces_the_bundle_into_a_new_folder_only() {
    let bundle: Map<String, Value> = [
        ("app.seed.rhai", &b"app;"[..]),
        ("pages/logo.png", &[0x89, 0x50, 0x00, 0xff]),
    ]
    .into_iter()
    .map(|(p, c)| (p.to_owned(), json!(BASE64.encode(c))))
    .collect();
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("out");
    export(&bundle, &target).unwrap();
    assert_eq!(
        std::fs::read(target.join("pages/logo.png")).unwrap(),
        [0x89, 0x50, 0x00, 0xff]
    );
    let files = read_folder_definition(&target).unwrap();
    assert_eq!(files.len(), 2);

    let err = export(&bundle, &target).unwrap_err();
    assert!(err.contains("not empty"), "{err}");
    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    export(&bundle, &empty).unwrap();
}

/// A repository holding a definition folder `def` that includes `key` from
/// `source`, with a shared file and a shared directory beside it.
fn repo(marker: &str, key: &str, source: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    if !marker.is_empty() {
        write(root, &format!("{marker}/HEAD"), b"x");
    }
    write(
        root,
        "apps/def/seedling.toml",
        toml::Table::from_iter([(
            "include".to_owned(),
            toml::Value::Table(toml::Table::from_iter([(
                key.to_owned(),
                toml::Value::String(source.to_owned()),
            )])),
        )])
        .to_string()
        .as_bytes(),
    );
    write(root, "apps/def/app.seed.rhai", b"app;");
    write(root, "apps/common/db.seed.rhai", b"db;");
    write(root, "apps/pages/500.html", b"<p>");
    write(root, "apps/pages/img/logo.png", b"png");
    dir
}

// spec: CLI#include-assembly
// spec: CLI#definition-export
#[test]
fn an_exported_bundle_with_includes_pushes_again_unchanged() {
    let dir = repo(".git", "errors", "../pages");
    let files = read_folder_definition(&dir.path().join("apps/def")).unwrap();
    let wire: Map<String, Value> = files
        .iter()
        .map(|(p, c)| (p.clone(), json!(BASE64.encode(c))))
        .collect();
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("exported");
    export(&wire, &target).unwrap();
    assert_eq!(read_folder_definition(&target).unwrap(), files);
}

// spec: CLI#definition-packing
#[tokio::test]
async fn a_packed_folder_is_the_bundle_a_push_would_send() {
    let dir = repo(".git", "errors", "../pages");
    let folder = dir.path().join("apps/def");
    let out = tempfile::tempdir().unwrap();
    let first = out.path().join("first.tar.gz");
    assert_eq!(pack(folder.to_str().unwrap(), &first).await.unwrap(), None);
    let packed = Bundle::from_tar_gz(&std::fs::read(&first).unwrap()).unwrap();
    let pushed = Bundle::from_files(read_folder_definition(&folder).unwrap()).unwrap();
    assert_eq!(packed.hash(), pushed.hash());

    let second = out.path().join("second.tar.gz");
    pack(folder.to_str().unwrap(), &second).await.unwrap();
    assert_eq!(
        std::fs::read(&first).unwrap(),
        std::fs::read(&second).unwrap()
    );
}

// spec: CLI#definition-packing
#[tokio::test]
async fn packing_reports_the_declared_version_requirement() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.seed.rhai", b"app;");
    write(dir.path(), "seedling.toml", b"seedling = \">=0.12, <0.14\"");
    let out = dir.path().join("out.tar.gz");
    let requirement = pack(dir.path().to_str().unwrap(), &out).await.unwrap();
    assert_eq!(requirement.as_deref(), Some(">=0.12, <0.14"));
}

// spec: CLI#definition-packing
#[tokio::test]
async fn packing_writes_nothing_for_a_broken_bundle_or_over_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "def/app.seed.rhai", b"app;");
    write(
        dir.path(),
        "def/seedling.toml",
        b"script = [\"missing.rhai\"]",
    );
    let out = dir.path().join("out.tar.gz");
    let e = pack(dir.path().join("def").to_str().unwrap(), &out)
        .await
        .unwrap_err();
    assert!(e.contains("missing.rhai"), "{e}");
    assert!(!out.exists());
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "nothing but the definition folder is left behind"
    );

    write(dir.path(), "def/seedling.toml", b"");
    write(dir.path(), "out.tar.gz", b"keep");
    let e = pack(dir.path().join("def").to_str().unwrap(), &out)
        .await
        .unwrap_err();
    assert!(e.contains("already exists"), "{e}");
    assert_eq!(std::fs::read(&out).unwrap(), b"keep");
}

// spec: CLI#definition-packing
#[tokio::test]
async fn a_packed_lone_file_keeps_its_name() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "web.seed.rhai", b"app;");
    let out = dir.path().join("out.tar.gz");
    pack(dir.path().join("web.seed.rhai").to_str().unwrap(), &out)
        .await
        .unwrap();
    let packed = Bundle::from_tar_gz(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(
        packed.script().unwrap().files().collect::<Vec<_>>(),
        ["web.seed.rhai"]
    );
}

// spec: CLI#definition-sources
#[tokio::test]
async fn a_source_breaking_the_bundle_rules_is_refused_before_it_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.seed.rhai", b"app;");
    write(dir.path(), "seedling.toml", b"seedling = 13");
    let args = DefinitionArgs {
        source: Some(dir.path().to_str().unwrap().to_owned()),
        reference: None,
    };
    let e = args.resolve().await.unwrap_err();
    assert!(e.contains("seedling"), "{e}");
}

// spec: CLI#definition-sources
#[tokio::test]
async fn the_bundle_demos_carry_the_files_they_share() {
    let demos = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demos");
    let helper = std::fs::read(demos.join("shared/static-site.rhai")).unwrap();
    let page = std::fs::read(demos.join("shared/www/shared.html")).unwrap();
    for name in ["bundle-tour", "bundle-share"] {
        let args = DefinitionArgs {
            source: Some(demos.join(name).to_str().unwrap().to_owned()),
            reference: None,
        };
        let Some(Definition::Bundle { bundle, origin }) = args.resolve().await.unwrap() else {
            panic!("{name} resolves to a bundle");
        };
        assert_eq!(origin, None);
        assert_eq!(
            bundle.file("lib/static-site.rhai").unwrap()[..],
            helper[..],
            "{name}"
        );
        assert_eq!(
            bundle.file("shared/shared.html").unwrap()[..],
            page[..],
            "{name}"
        );
    }
}
