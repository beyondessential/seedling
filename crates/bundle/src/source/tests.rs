use std::io::Write;

use super::*;
use crate::Bundle;

fn write(root: &Path, rel: &str, contents: &[u8]) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn tarball(build: impl FnOnce(&mut tar::Builder<Vec<u8>>)) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    build(&mut builder);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&builder.into_inner().unwrap()).unwrap();
    gz.finish().unwrap()
}

fn tar_file(b: &mut tar::Builder<Vec<u8>>, path: &str, contents: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::Regular);
    b.append_data(&mut header, path, contents).unwrap();
}

fn tar_link(b: &mut tar::Builder<Vec<u8>>, kind: tar::EntryType, path: &str, target: &str) {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_size(0);
    b.append_link(&mut header, path, target).unwrap();
}

// spec: CLI#definition-folders
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
    for dir in [".hg", ".svn", ".bzr", "CVS", "_darcs", ".pijul", ".sl"] {
        write(root, &format!("{dir}/state"), b"x");
    }
    write(root, "pages/.git", b"gitdir: elsewhere");
    write(root, ".seedignore", b"notes/\npages/draft.html\n");
    let files = read_folder(&Disk::new(root), "", &mut Budget::full()).unwrap();
    assert_eq!(
        files.keys().collect::<Vec<_>>(),
        vec![".seedignore", "app.seed.rhai", "pages/500.html"]
    );
}

// spec: CLI#definition-folders
#[test]
#[cfg(unix)]
fn a_symlink_is_reported_and_nothing_is_read() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.seed.rhai", b"app;");
    std::os::unix::fs::symlink("/etc/passwd", dir.path().join("evil")).unwrap();
    let err = read_folder(&Disk::new(dir.path()), "", &mut Budget::full()).unwrap_err();
    assert!(err.contains("evil"), "{err}");
    // An ignored symlink is simply left out.
    write(dir.path(), ".seedignore", b"evil\n");
    read_folder(&Disk::new(dir.path()), "", &mut Budget::full()).unwrap();
}

// spec: CLI#definition-sources
#[test]
fn a_github_tarball_yields_the_named_folder() {
    let data = tarball(|b| {
        tar_file(b, "org-repo-abc123/README.md", b"top");
        tar_file(b, "org-repo-abc123/apps/web/app.seed.rhai", b"app;");
        tar_file(b, "org-repo-abc123/apps/web/pages/500.html", b"<p>");
        tar_file(b, "org-repo-abc123/apps/other/app.seed.rhai", b"no");
    });
    let files = read_archive_definition(&data, "apps/web", None)
        .unwrap()
        .files;
    assert_eq!(
        files.keys().collect::<Vec<_>>(),
        vec!["app.seed.rhai", "pages/500.html"]
    );
}

// spec: CLI#definition-sources
#[test]
fn a_lone_file_is_a_bundle_holding_it_under_its_own_name() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "tamanu central.seed.rhai", b"app;");
    let files = read_lone_file(&dir.path().join("tamanu central.seed.rhai")).unwrap();
    assert_eq!(files["tamanu central.seed.rhai"], b"app;");
    let metadata: toml::Table =
        toml::from_str(std::str::from_utf8(&files["seedling.toml"]).unwrap()).unwrap();
    assert_eq!(
        metadata,
        toml::from_str::<toml::Table>(r#"script = ["tamanu central.seed.rhai"]"#).unwrap()
    );
    assert_eq!(files.len(), 2);
    crate::Bundle::from_files(files).unwrap().script().unwrap();
}

// spec: CLI#definition-sources
#[test]
fn a_lone_seedling_toml_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "seedling.toml", b"");
    let err = read_lone_file(&dir.path().join("seedling.toml")).unwrap_err();
    assert!(err.contains("seedling.toml"), "{err}");
}

// spec: CLI#definition-sources
#[test]
#[cfg(unix)]
fn a_non_utf8_name_is_refused() {
    use std::os::unix::ffi::OsStrExt;

    let dir = tempfile::tempdir().unwrap();
    let odd = dir
        .path()
        .join(std::ffi::OsStr::from_bytes(b"app-\xff.rhai"));
    std::fs::write(&odd, b"app;").unwrap();
    let err = read_lone_file(&odd).unwrap_err();
    assert!(err.contains("not valid UTF-8"), "{err}");
}

// spec: CLI#definition-sources
// spec: CLI#definition-folders
#[test]
fn a_tarball_entry_may_not_escape_the_repository_or_be_a_link_in_the_folder() {
    // The builder refuses to write `..`, so the name goes into the header
    // raw, the way a hostile archive would carry it.
    let escaping = tarball(|b| {
        tar_file(b, "org-repo-abc/app.seed.rhai", b"app;");
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        let raw = b"org-repo-abc/../../escaped";
        header.as_gnu_mut().unwrap().name[..raw.len()].copy_from_slice(raw);
        header.set_cksum();
        b.append(&header, &b"owned"[..]).unwrap();
    });
    let e = read_archive_definition(&escaping, "", None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("outside the repository"), "{e}");

    // Dropping GitHub's prefix directory would otherwise drop a leading
    // `..` with it, and write the entry inside the repository.
    let leading = tarball(|b| {
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        let raw = b"../planted";
        header.as_gnu_mut().unwrap().name[..raw.len()].copy_from_slice(raw);
        header.set_cksum();
        b.append(&header, &b"owned"[..]).unwrap();
    });
    let e = read_archive_definition(&leading, "", None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("outside the repository"), "{e}");

    for kind in [tar::EntryType::Symlink, tar::EntryType::Link] {
        let linked = tarball(|b| {
            tar_file(b, "org-repo-abc/apps/web/app.seed.rhai", b"app;");
            tar_link(
                b,
                kind,
                "org-repo-abc/apps/web/stolen",
                "../../../../etc/passwd",
            );
        });
        let e = read_archive_definition(&linked, "apps/web", None)
            .unwrap_err()
            .to_string();
        assert!(e.contains("not a regular file"), "{e}");
    }
}

// spec: CLI#definition-sources
// spec: CLI#include-assembly
#[test]
fn a_github_folder_includes_from_elsewhere_in_the_repository() {
    let data = tarball(|b| {
        tar_file(
            b,
            "o-r-abc/apps/web/seedling.toml",
            b"script = [\"lib/db.seed.rhai\", \"app.seed.rhai\"]\n[include]\n\"lib\" = \"../common\"",
        );
        tar_file(b, "o-r-abc/apps/web/app.seed.rhai", b"app;");
        tar_file(b, "o-r-abc/apps/common/db.seed.rhai", b"db;");
        tar_link(b, tar::EntryType::Symlink, "o-r-abc/docs/latest", "v2");
    });
    let files = read_archive_definition(&data, "apps/web", None)
        .unwrap()
        .files;
    assert_eq!(files["lib/db.seed.rhai"], b"db;");

    let escaping = tarball(|b| {
        tar_file(
            b,
            "o-r-abc/apps/web/seedling.toml",
            b"[include]\n\"lib\" = \"../../../db.seed.rhai\"",
        );
    });
    let e = read_archive_definition(&escaping, "apps/web", None)
        .unwrap_err()
        .to_string();
    assert!(e.ends_with("lies outside the repository"), "{e}");
}

// spec: CLI#definition-sources
// spec: CLI#include-assembly
#[test]
fn a_github_include_through_a_link_or_beneath_one_is_refused() {
    let through = tarball(|b| {
        tar_file(
            b,
            "o-r-abc/apps/web/seedling.toml",
            b"[include]\n\"lib\" = \"../shared/db.seed.rhai\"",
        );
        tar_link(b, tar::EntryType::Symlink, "o-r-abc/apps/shared", "/etc");
    });
    let e = read_archive_definition(&through, "apps/web", None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("apps/shared is a symlink"), "{e}");

    let beneath = tarball(|b| {
        tar_link(b, tar::EntryType::Symlink, "o-r-abc/apps/shared", "/etc");
        tar_file(b, "o-r-abc/apps/shared/passwd", b"x");
    });
    let e = read_archive_definition(&beneath, "apps", None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("apps/shared is not a regular file"), "{e}");

    // The same archive with the link last is refused the same way, before
    // anything is written.
    let link_last = tarball(|b| {
        tar_file(b, "o-r-abc/apps/shared/passwd", b"x");
        tar_link(b, tar::EntryType::Symlink, "o-r-abc/apps/shared", "/etc");
    });
    let e = read_archive_definition(&link_last, "apps", None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("apps/shared is not a regular file"), "{e}");
}

// spec: CLI#definition-sources
#[test]
fn a_github_folder_beneath_a_link_is_refused() {
    let data = tarball(|b| {
        tar_link(b, tar::EntryType::Symlink, "o-r-abc/apps", "elsewhere");
    });
    let e = read_archive_definition(&data, "apps/web", None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("apps is a symlink"), "{e}");
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

fn assembled(dir: &tempfile::TempDir) -> Result<BTreeMap<String, Vec<u8>>, String> {
    read_folder_definition(&dir.path().join("apps/def"), None)
        .map(|a| a.files)
        .map_err(|e| e.to_string())
}

// spec: CLI#include-assembly
#[test]
fn files_and_directories_are_included_from_the_repository() {
    let files = assembled(&repo(".git", "lib/db.seed.rhai", "../common/db.seed.rhai")).unwrap();
    assert_eq!(files["lib/db.seed.rhai"], b"db;");

    let files = assembled(&repo(".jj", "errors", "../pages/")).unwrap();
    assert_eq!(files["errors/500.html"], b"<p>");
    assert_eq!(files["errors/img/logo.png"], b"png");

    let files = assembled(&repo(".git", "copy.seed.rhai", "./app.seed.rhai")).unwrap();
    assert_eq!(files["copy.seed.rhai"], b"app;");
}

// spec: CLI#include-assembly
#[test]
fn without_a_repository_includes_stay_within_the_folder() {
    let dir = repo("", "lib/db.seed.rhai", "../common/db.seed.rhai");
    let e = assembled(&dir).unwrap_err();
    assert!(e.contains("lies outside the definition folder"), "{e}");
    assembled(&repo("", "copy.seed.rhai", "app.seed.rhai")).unwrap();
}

// spec: CLI#include-assembly
#[test]
fn an_out_of_bounds_source_is_never_read() {
    for source in [
        "../../../outside",
        "/etc/hostname",
        "../common/\0db.seed.rhai",
        "../../.git/HEAD",
    ] {
        let dir = repo(".git", "lib/x", source);
        let e = assembled(&dir).unwrap_err();
        assert!(e.contains("lies outside"), "{source}: {e}");
        // An exported bundle carries its own copy, which stands in for a
        // source it cannot reach.
        write(dir.path(), "apps/def/lib/x", b"kept");
        assert_eq!(assembled(&dir).unwrap()["lib/x"], b"kept", "{source}");
    }
}

// spec: CLI#include-assembly
#[test]
#[cfg(unix)]
fn a_symlink_below_the_boundary_is_refused_and_one_above_it_is_not() {
    let dir = repo(".git", "lib/db.seed.rhai", "../linked/db.seed.rhai");
    std::os::unix::fs::symlink(
        dir.path().join("apps/common"),
        dir.path().join("apps/linked"),
    )
    .unwrap();
    let e = assembled(&dir).unwrap_err();
    assert!(e.contains("apps/linked is a symlink"), "{e}");

    let outer = tempfile::tempdir().unwrap();
    let inner = repo(".git", "lib/db.seed.rhai", "../common/db.seed.rhai");
    let via = outer.path().join("via");
    std::os::unix::fs::symlink(inner.path(), &via).unwrap();
    let files = read_folder_definition(&via.join("apps/def"), None)
        .unwrap()
        .files;
    assert_eq!(files["lib/db.seed.rhai"], b"db;");
}

// spec: CLI#include-assembly
#[test]
fn a_directory_source_must_yield_files_and_no_metadata_file() {
    let dir = repo(".git", "errors", "../empty");
    std::fs::create_dir_all(dir.path().join("apps/empty")).unwrap();
    let e = assembled(&dir).unwrap_err();
    assert!(e.contains("holds no files"), "{e}");

    let dir = repo(".git", "errors", "../pages");
    write(dir.path(), "apps/pages/img/seedling.toml", b"");
    let e = assembled(&dir).unwrap_err();
    assert!(e.contains("img/seedling.toml"), "{e}");
}

// spec: CLI#include-assembly
#[test]
fn a_copy_in_the_folder_must_match_its_source_and_stands_in_for_a_missing_one() {
    let dir = repo(".git", "lib/db.seed.rhai", "../common/db.seed.rhai");
    write(dir.path(), "apps/def/lib/db.seed.rhai", b"db;");
    assembled(&dir).unwrap();

    write(dir.path(), "apps/def/lib/db.seed.rhai", b"stale;");
    let e = assembled(&dir).unwrap_err();
    assert!(
        e.contains("differs from its source at lib/db.seed.rhai"),
        "{e}"
    );

    std::fs::remove_file(dir.path().join("apps/common/db.seed.rhai")).unwrap();
    assert_eq!(assembled(&dir).unwrap()["lib/db.seed.rhai"], b"stale;");

    std::fs::remove_file(dir.path().join("apps/def/lib/db.seed.rhai")).unwrap();
    let e = assembled(&dir).unwrap_err();
    assert!(e.contains("names nothing"), "{e}");

    let dir = repo(".git", "errors", "../pages");
    write(dir.path(), "apps/def/errors/500.html", b"<p>");
    let e = assembled(&dir).unwrap_err();
    assert!(
        e.contains("differs from its source at errors/img/logo.png"),
        "a partial copy differs where it lacks a file: {e}"
    );
}

// spec: CLI#include-assembly
#[test]
fn an_included_path_is_placed_even_where_seedignore_matches_it() {
    let dir = repo(".git", "lib/db.seed.rhai", "../common/db.seed.rhai");
    write(dir.path(), "apps/def/.seedignore", b"lib/\n");
    assert_eq!(assembled(&dir).unwrap()["lib/db.seed.rhai"], b"db;");
}

// spec: CLI#definition-sources
#[test]
fn a_lone_files_script_errors_name_the_file() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "web.seed.rhai", b"let x = 1;\nlet y = ;\n");
    let bundle =
        Bundle::from_files(read_lone_file(&dir.path().join("web.seed.rhai")).unwrap()).unwrap();
    let message = bundle
        .script()
        .unwrap()
        .remap_error("Unexpected ';' (line 2, position 9)");
    assert!(message.contains("web.seed.rhai line 2"), "{message}");
}

// spec: CLI#definition-sources
#[test]
fn a_github_url_folder_may_not_climb_out_of_the_repository() {
    let data = tarball(|b| tar_file(b, "o-r-abc/apps/web/app.seed.rhai", b"app;"));
    let e = read_archive_definition(&data, "apps/../../outside", None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("outside the repository"), "{e}");
    let files = read_archive_definition(&data, "apps/./web/", None)
        .unwrap()
        .files;
    assert_eq!(files["app.seed.rhai"], b"app;");
}

// spec: CLI#include-assembly
#[test]
fn a_file_beside_an_included_path_is_not_taken_for_a_copy_of_it() {
    let dir = repo(".git", "errors", "../pages");
    write(dir.path(), "apps/def/errors-old.html", b"old");
    write(dir.path(), "apps/def/errors.txt", b"note");
    let files = assembled(&dir).unwrap();
    assert_eq!(files["errors/500.html"], b"<p>");
    assert_eq!(files["errors-old.html"], b"old");
}

// spec: CLI#include-assembly
#[test]
fn a_source_larger_than_any_bundle_is_refused_as_it_is_read() {
    let dir = repo(".git", "everything", "../..");
    for i in 0..=crate::BUNDLE_FILE_LIMIT {
        write(dir.path(), &format!("many/{i}"), b"");
    }
    let e = assembled(&dir).unwrap_err();
    assert!(e.contains("past the 4096 files"), "{e}");

    let dir = repo(".git", "big.bin", "../big.bin");
    let big = vec![0u8; crate::BUNDLE_SIZE_LIMIT + 1];
    write(dir.path(), "apps/big.bin", &big);
    let e = assembled(&dir).unwrap_err();
    assert!(e.contains("size limit"), "{e}");
}

// spec: CLI#definition-sources
#[test]
fn an_archive_entry_named_as_a_link_stays_a_link() {
    let data = tarball(|b| {
        tar_link(b, tar::EntryType::Symlink, "o-r-abc/apps/shared", "/etc");
        tar_file(b, "o-r-abc/apps/shared", b"shadow");
    });
    let e = read_archive_definition(&data, "apps", None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("apps/shared is not a regular file"), "{e}");
}

// spec: CLI#include-assembly
#[test]
fn the_folder_and_its_sources_share_one_budget() {
    let half = vec![0u8; crate::BUNDLE_SIZE_LIMIT / 2 + 1];
    let dir = repo(".git", "big.bin", "../big.bin");
    write(dir.path(), "apps/big.bin", &half);
    write(dir.path(), "apps/def/also-big.bin", &half);
    let e = assembled(&dir).unwrap_err();
    assert!(e.contains("size limit"), "{e}");

    // A copy matching its source is counted once, not twice.
    let dir = repo(".git", "big.bin", "../big.bin");
    write(dir.path(), "apps/big.bin", &half);
    write(dir.path(), "apps/def/big.bin", &half);
    assert_eq!(assembled(&dir).unwrap()["big.bin"], half);
}

// spec: CLI#definition-sources
#[test]
fn a_lone_file_larger_than_any_bundle_is_refused_as_it_is_read() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "big.seed.rhai",
        &vec![b' '; crate::BUNDLE_SIZE_LIMIT + 1],
    );
    let e = read_lone_file(&dir.path().join("big.seed.rhai")).unwrap_err();
    assert!(e.contains("size limit"), "{e}");
}

// spec: CLI#definition-sources
// spec: CLI#include-assembly
#[test]
fn only_the_definition_and_its_sources_are_unpacked_from_a_github_archive() {
    let big = vec![0u8; crate::BUNDLE_SIZE_LIMIT + 1];
    let data = tarball(|b| {
        tar_file(
            b,
            "o-r-abc/apps/web/seedling.toml",
            b"[include]\n\"lib\" = \"../common\"",
        );
        tar_file(b, "o-r-abc/apps/web/app.seed.rhai", b"app;");
        tar_file(b, "o-r-abc/apps/common/db.seed.rhai", b"db;");
        tar_file(b, "o-r-abc/assets/huge.bin", &big);
        tar_file(b, "o-r-abc/README.md", b"readme");
    });
    let files = read_archive_definition(&data, "apps/web", None)
        .unwrap()
        .files;
    assert_eq!(files["lib/db.seed.rhai"], b"db;");

    let wanting_it = tarball(|b| {
        tar_file(b, "o-r-abc/apps/web/app.seed.rhai", b"app;");
        tar_file(b, "o-r-abc/apps/web/huge.bin", &big);
    });
    let e = read_archive_definition(&wanting_it, "apps/web", None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("size limit"), "{e}");
}

// spec: CLI#include-assembly
#[test]
fn a_folder_at_its_repository_root_is_told_it_is_in_one() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".git/HEAD", b"x");
    write(dir.path(), "app.seed.rhai", b"app;");
    write(
        dir.path(),
        "seedling.toml",
        b"[include]\n\"lib\" = \"../lib\"",
    );
    let e = read_folder_definition(dir.path(), None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("lies outside the repository at"), "{e}");
}

// spec: CLI#definition-sources
#[test]
fn a_non_utf8_path_fails_only_a_definition_that_reads_it() {
    fn odd_name(b: &mut tar::Builder<Vec<u8>>, raw: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.as_gnu_mut().unwrap().name[..raw.len()].copy_from_slice(raw);
        header.set_cksum();
        b.append(&header, &b"x"[..]).unwrap();
    }
    let elsewhere = tarball(|b| {
        tar_file(b, "o-r-abc/apps/web/app.seed.rhai", b"app;");
        odd_name(b, b"o-r-abc/docs/caf\xe9.txt");
    });
    read_archive_definition(&elsewhere, "apps/web", None).unwrap();

    let within = tarball(|b| {
        tar_file(b, "o-r-abc/apps/web/app.seed.rhai", b"app;");
        odd_name(b, b"o-r-abc/apps/web/caf\xe9.txt");
    });
    let e = read_archive_definition(&within, "apps/web", None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("non-UTF-8"), "{e}");
}

/// The same repository on disk and as a GitHub archive, with the definition
/// at `apps/def`.
fn both_ways(files: &[(&str, &[u8])]) -> (tempfile::TempDir, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".git/HEAD", b"x");
    for (path, contents) in files {
        write(dir.path(), path, contents);
    }
    let data = tarball(|b| {
        for (path, contents) in files {
            tar_file(b, &format!("o-r-abc/{path}"), contents);
        }
    });
    (dir, data)
}

// spec: CLI#definition-folders
// spec: CLI#include-assembly
#[test]
fn a_definition_reads_the_same_from_disk_and_from_github() {
    let (dir, data) = both_ways(&[
        (
            "apps/def/seedling.toml",
            b"[include]\n\"lib\" = \"../common\"\n\"pages\" = \"../pages\"",
        ),
        ("apps/def/app.seed.rhai", b"app;"),
        ("apps/def/.seedignore", b"*.md\ndrafts/\n"),
        ("apps/def/notes.md", b"left out"),
        ("apps/def/drafts/wip.html", b"left out"),
        ("apps/def/drafts/.seedignore", b"!wip.html\n"),
        ("apps/def/sub/.seedignore", b"!keep.md\n"),
        ("apps/def/sub/keep.md", b"kept again"),
        ("apps/def/sub/other.md", b"left out"),
        ("apps/common/db.seed.rhai", b"db;"),
        (
            "apps/common/README.md",
            b"kept: the folder's rules stop at the folder",
        ),
        ("apps/pages/.seedignore", b"*.tmp\n"),
        ("apps/pages/500.html", b"<p>"),
        ("apps/pages/cache.tmp", b"left out"),
        ("README.md", b"elsewhere"),
    ]);
    let from_disk = read_folder_definition(&dir.path().join("apps/def"), None)
        .unwrap()
        .files;
    let from_github = read_archive_definition(&data, "apps/def", None)
        .unwrap()
        .files;
    assert_eq!(from_disk, from_github);
    assert_eq!(
        from_disk.keys().collect::<Vec<_>>(),
        vec![
            ".seedignore",
            "app.seed.rhai",
            "lib/README.md",
            "lib/db.seed.rhai",
            "pages/.seedignore",
            "pages/500.html",
            "seedling.toml",
            "sub/.seedignore",
            "sub/keep.md",
        ]
    );
}

// spec: CLI#definition-folders
#[test]
fn a_link_a_definition_leaves_out_is_no_bar_from_github() {
    let data = tarball(|b| {
        tar_file(b, "o-r-abc/apps/web/app.seed.rhai", b"app;");
        tar_file(b, "o-r-abc/apps/web/.seedignore", b"latest\n");
        tar_link(b, tar::EntryType::Symlink, "o-r-abc/apps/web/latest", "v2");
    });
    let files = read_archive_definition(&data, "apps/web", None)
        .unwrap()
        .files;
    assert!(!files.contains_key("latest"));
}

// spec: CLI#include-assembly
#[test]
fn what_a_github_definition_leaves_out_or_repeats_costs_nothing() {
    let most = vec![1u8; crate::BUNDLE_SIZE_LIMIT * 3 / 4];
    let data = tarball(|b| {
        tar_file(
            b,
            "o-r-abc/apps/web/seedling.toml",
            b"[include]\n\"big.bin\" = \"../shared/big.bin\"",
        );
        tar_file(b, "o-r-abc/apps/web/app.seed.rhai", b"app;");
        tar_file(b, "o-r-abc/apps/web/.seedignore", b"fixtures/\n");
        tar_file(b, "o-r-abc/apps/web/fixtures/huge.bin", &most);
        tar_file(b, "o-r-abc/apps/web/fixtures/huger.bin", &most);
        // A copy already in the folder, beside its source.
        tar_file(b, "o-r-abc/apps/web/big.bin", &most);
        tar_file(b, "o-r-abc/apps/shared/big.bin", &most);
    });
    let files = read_archive_definition(&data, "apps/web", None)
        .unwrap()
        .files;
    assert_eq!(files["big.bin"], most);
    assert!(!files.keys().any(|k| k.starts_with("fixtures")));
}

// spec: CLI#definition-sources
#[test]
fn a_github_definition_at_the_repository_root_reads_the_whole_repository() {
    let data = tarball(|b| {
        tar_file(b, "o-r-abc/app.seed.rhai", b"app;");
        tar_file(b, "o-r-abc/pages/500.html", b"<p>");
    });
    let files = read_archive_definition(&data, "", None).unwrap().files;
    assert_eq!(
        files.keys().collect::<Vec<_>>(),
        vec!["app.seed.rhai", "pages/500.html"]
    );
}

// spec: CLI#include-assembly
#[test]
fn many_includes_read_from_github_as_few_do() {
    let keys = crate::BUNDLE_FILE_LIMIT / 2;
    let mut metadata = String::from("[include]\n");
    for i in 0..keys {
        metadata.push_str(&format!("\"lib/{i}\" = \"../shared/{i}\"\n"));
    }
    let data = tarball(|b| {
        tar_file(b, "o-r-abc/apps/web/seedling.toml", metadata.as_bytes());
        tar_file(b, "o-r-abc/apps/web/app.seed.rhai", b"app;");
        for i in 0..keys {
            tar_file(
                b,
                &format!("o-r-abc/apps/shared/{i}"),
                i.to_string().as_bytes(),
            );
            tar_file(b, &format!("o-r-abc/docs/{i}.md"), b"elsewhere");
        }
    });
    let files = read_archive_definition(&data, "apps/web", None)
        .unwrap()
        .files;
    assert_eq!(
        files[&format!("lib/{}", keys - 1)],
        (keys - 1).to_string().as_bytes()
    );
    assert!(!files.keys().any(|k| k.ends_with(".md")));
}

// spec: CLI#definition-sources
#[test]
fn a_deeply_nested_github_path_is_read_without_exhausting_the_stack() {
    // Deeper than a test thread's stack would allow one frame per level.
    let deep = vec!["d"; 5_000].join("/");
    let data = tarball(|b| {
        tar_file(b, "o-r-abc/apps/web/app.seed.rhai", b"app;");
        let path = format!("o-r-abc/apps/web/{deep}/x");
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        b.append_data(&mut header, path, &b"x"[..]).unwrap();
    });
    let files = read_archive_definition(&data, "apps/web", None)
        .unwrap()
        .files;
    assert_eq!(files[&format!("{deep}/x")], b"x");
}

// spec: CLI#definition-folders
#[test]
fn a_non_utf8_path_a_github_definition_leaves_out_is_no_bar() {
    let data = tarball(|b| {
        tar_file(b, "o-r-abc/apps/web/app.seed.rhai", b"app;");
        tar_file(b, "o-r-abc/apps/web/.seedignore", b"scratch/\n");
        let raw = b"o-r-abc/apps/web/scratch/caf\xe9.txt";
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.as_gnu_mut().unwrap().name[..raw.len()].copy_from_slice(raw);
        header.set_cksum();
        b.append(&header, &b"x"[..]).unwrap();
    });
    let files = read_archive_definition(&data, "apps/web", None)
        .unwrap()
        .files;
    assert_eq!(
        files.keys().collect::<Vec<_>>(),
        [".seedignore", "app.seed.rhai"]
    );
}

// spec: CLI#include-assembly
#[test]
fn a_copy_beside_its_source_counts_once_against_the_file_limit_from_github() {
    // Each half of the limit, held twice: once in the folder, once in the
    // source it was copied from.
    let half = crate::BUNDLE_FILE_LIMIT / 2;
    let data = tarball(|b| {
        tar_file(
            b,
            "o-r-abc/apps/web/seedling.toml",
            b"[include]\n\"lib\" = \"../shared\"",
        );
        tar_file(b, "o-r-abc/apps/web/app.seed.rhai", b"app;");
        for i in 0..half {
            let contents = i.to_string();
            tar_file(b, &format!("o-r-abc/apps/web/lib/{i}"), contents.as_bytes());
            tar_file(b, &format!("o-r-abc/apps/shared/{i}"), contents.as_bytes());
        }
    });
    let files = read_archive_definition(&data, "apps/web", None)
        .unwrap()
        .files;
    assert_eq!(files.len(), half + 2);
}

// spec: CLI#definition-folders
// spec: CLI#include-assembly
#[test]
fn an_empty_source_folder_reads_the_same_from_disk_and_from_github() {
    let (dir, _) = both_ways(&[
        (
            "apps/def/seedling.toml",
            b"[include]\n\"lib\" = \"../empty\"",
        ),
        ("apps/def/app.seed.rhai", b"app;"),
    ]);
    std::fs::create_dir_all(dir.path().join("apps/empty")).unwrap();
    let data = tarball(|b| {
        tar_file(
            b,
            "o-r-abc/apps/def/seedling.toml",
            b"[include]\n\"lib\" = \"../empty\"",
        );
        tar_file(b, "o-r-abc/apps/def/app.seed.rhai", b"app;");
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        b.append_data(&mut header, "o-r-abc/apps/empty/", &[][..])
            .unwrap();
    });
    let from_disk = read_folder_definition(&dir.path().join("apps/def"), None)
        .unwrap_err()
        .to_string();
    let from_github = read_archive_definition(&data, "apps/def", None)
        .unwrap_err()
        .to_string();
    assert!(from_disk.ends_with("holds no files"), "{from_disk}");
    assert!(from_github.ends_with("holds no files"), "{from_github}");
}
