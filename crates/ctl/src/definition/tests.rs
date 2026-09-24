use std::io::Write;

use super::*;

fn write(root: &Path, rel: &str, contents: &[u8]) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

// i[verify ctl.definition.folder]
#[test]
fn folders_skip_vcs_metadata_and_seedignored_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "app.seed.rhai", b"app;");
    write(root, "pages/500.html", b"<p>");
    write(root, "pages/draft.html", b"wip");
    write(root, "notes/todo.md", b"x");
    write(root, ".git/HEAD", b"ref");
    write(root, ".jj/repo", b"x");
    write(root, ".seedignore", b"notes/\npages/draft.html\n");
    let files = read_folder(root).unwrap();
    assert_eq!(
        files.keys().collect::<Vec<_>>(),
        vec![".seedignore", "app.seed.rhai", "pages/500.html"]
    );
}

// i[verify ctl.definition.folder]
#[test]
fn a_symlink_is_reported_and_nothing_is_read() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.seed.rhai", b"app;");
    std::os::unix::fs::symlink("/etc/passwd", dir.path().join("evil")).unwrap();
    let err = read_folder(dir.path()).unwrap_err();
    assert!(err.contains("evil"), "{err}");
    // An ignored symlink is simply left out.
    write(dir.path(), ".seedignore", b"evil\n");
    read_folder(dir.path()).unwrap();
}

// i[verify ctl.definition.source]
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

// i[verify ctl.definition.source]
#[test]
fn a_github_tarball_yields_the_named_folder() {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, contents) in [
        ("org-repo-abc123/README.md", &b"top"[..]),
        ("org-repo-abc123/apps/web/app.seed.rhai", b"app;"),
        ("org-repo-abc123/apps/web/pages/500.html", b"<p>"),
        ("org-repo-abc123/apps/other/app.seed.rhai", b"no"),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        builder.append_data(&mut header, path, contents).unwrap();
    }
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&builder.into_inner().unwrap()).unwrap();
    let tarball = gz.finish().unwrap();

    let dir = tempfile::tempdir().unwrap();
    github::extract_folder(&tarball, "apps/web", dir.path()).unwrap();
    let files = read_folder(dir.path()).unwrap();
    assert_eq!(
        files.keys().collect::<Vec<_>>(),
        vec!["app.seed.rhai", "pages/500.html"]
    );
}

// i[verify ctl.definition.source]
#[test]
fn a_pushed_bundle_carries_its_origin() {
    let def = Definition::Bundle {
        files: BTreeMap::from([("app.seed.rhai".to_owned(), b"app;".to_vec())]),
        origin: Some(("https://github.com/o/r/tree/main/x".into(), "abc".into())),
    };
    let fields = def.fields(DefinitionKeys::APP);
    assert_eq!(fields["bundle"]["app.seed.rhai"], BASE64.encode(b"app;"));
    assert_eq!(fields["origin"]["revision"], "abc");
    assert_eq!(
        Definition::Script("x".into()).fields(DefinitionKeys::TEMPLATE),
        json!({ "body": "x" }).as_object().unwrap().clone()
    );
    assert_eq!(
        Definition::Reference("ghcr.io/o/d:1".into()).fields(DefinitionKeys::APP)["reference"],
        "ghcr.io/o/d:1"
    );
}

// i[verify ctl.definition.param]
#[test]
fn one_param_change_rides_along() {
    assert_eq!(
        param_change(Some("version=v2.12"), None).unwrap(),
        Some(json!({ "name": "version", "value": "v2.12" }))
    );
    assert_eq!(
        param_change(None, Some("version")).unwrap(),
        Some(json!({ "name": "version", "value": null }))
    );
    assert!(param_change(Some("bare"), None).is_err());
    assert!(param_change(Some("a=b"), Some("c")).is_err());
}

// i[verify ctl.definition.export]
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
    let files = read_folder(&target).unwrap();
    assert_eq!(files.len(), 2);

    let err = export(&bundle, &target).unwrap_err();
    assert!(err.contains("not empty"), "{err}");
    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    export(&bundle, &empty).unwrap();
}

// i[verify ctl.definition.source]
// i[verify ctl.definition.folder]
#[test]
fn a_tarball_entry_may_not_escape_the_folder_or_be_a_link() {
    fn tarball(build: impl FnOnce(&mut tar::Builder<Vec<u8>>)) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        build(&mut builder);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&builder.into_inner().unwrap()).unwrap();
        gz.finish().unwrap()
    }
    fn file(b: &mut tar::Builder<Vec<u8>>, path: &str, contents: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        b.append_data(&mut header, path, contents).unwrap();
    }

    // A whole-repository URL leaves no folder prefix to filter on. The
    // builder refuses to write `..`, so the name goes into the header raw,
    // the way a hostile archive would carry it.
    let escaping = tarball(|b| {
        file(b, "org-repo-abc/app.seed.rhai", b"app;");
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        let raw = b"org-repo-abc/../../escaped";
        header.as_gnu_mut().unwrap().name[..raw.len()].copy_from_slice(raw);
        header.set_cksum();
        b.append(&header, &b"owned"[..]).unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let e = github::extract_folder(&escaping, "", dir.path()).unwrap_err();
    assert!(e.contains("outside the folder"), "{e}");
    assert!(!dir.path().parent().unwrap().join("escaped").exists());

    for kind in [tar::EntryType::Symlink, tar::EntryType::Link] {
        let linked = tarball(|b| {
            file(b, "org-repo-abc/apps/web/app.seed.rhai", b"app;");
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(kind);
            header.set_size(0);
            b.append_link(
                &mut header,
                "org-repo-abc/apps/web/stolen",
                "../../../../etc/passwd",
            )
            .unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let e = github::extract_folder(&linked, "apps/web", dir.path()).unwrap_err();
        assert!(e.contains("not a regular file"), "{e}");
        assert!(!dir.path().join("stolen").exists());
    }
}
