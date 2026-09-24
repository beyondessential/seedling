use semver::Version;

use super::{testing::FakeRegistry, *};

const REPO: &str = "ghcr.io/org/app-def";

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

fn r(s: &str) -> DefinitionRef {
    DefinitionRef::parse(s).unwrap()
}

fn block<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(f)
}

fn meta(req: &str) -> Vec<u8> {
    format!("seedling = \"{req}\"").into_bytes()
}

// i[verify definition.source]
#[test]
fn references_must_be_complete() {
    for bad in ["app-def:1", "org/app-def:1", "ghcr.io/org/app-def"] {
        assert!(
            matches!(DefinitionRef::parse(bad), Err(FetchError::BadReference(_))),
            "{bad}"
        );
    }
    let ok = r("ghcr.io/org/app-def:1.2");
    assert_eq!(ok.registry(), "ghcr.io");
    assert_eq!(ok.repository(), "org/app-def");
    r("localhost:5000/app@sha256:0000000000000000000000000000000000000000000000000000000000000000");
}

// i[verify definition.fetch]
// i[verify definition.provenance]
#[test]
fn standalone_artefact_is_fetched_with_its_manifest_digest() {
    let reg = FakeRegistry::default();
    let digest = reg.push_definition(
        REPO,
        &[("app.seed.rhai", b"app;"), ("pages/500.html", b"<p>")],
        None,
    );
    reg.tag(REPO, "1", &digest);
    let got = block(fetch(&reg, &r("ghcr.io/org/app-def:1"), &v("0.12.0"))).unwrap();
    assert_eq!(got.digest, digest);
    assert!(got.bundle.file("pages/500.html").is_some());
}

// i[verify definition.fetch]
#[test]
fn index_entry_is_selected_and_its_digest_recorded() {
    let reg = FakeRegistry::default();
    let image = reg.push_image(REPO);
    let def = reg.push_definition(REPO, &[("app.seed.rhai", b"app;")], None);
    let index = reg.push_index(
        REPO,
        &[(&image, None, None), (&def, Some(ARTIFACT_TYPE), None)],
    );
    reg.tag(REPO, "2.12", &index);
    let got = block(fetch(&reg, &r("ghcr.io/org/app-def:2.12"), &v("0.12.0"))).unwrap();
    assert_eq!(got.digest, def);
    assert_ne!(got.digest, index);
}

// i[verify definition.fetch]
#[test]
fn a_reference_naming_no_definition_fails() {
    let reg = FakeRegistry::default();
    let image = reg.push_image(REPO);
    reg.tag(REPO, "img", &image);
    let index = reg.push_index(REPO, &[(&image, None, None)]);
    reg.tag(REPO, "idx", &index);
    for tag in ["img", "idx", "missing"] {
        let err = block(fetch(&reg, &r(&format!("{REPO}:{tag}")), &v("0.12.0"))).unwrap_err();
        assert!(matches!(err, FetchError::Failed(_)), "{tag}: {err:?}");
    }
    reg.set_unreachable(true);
    let err = block(fetch(&reg, &r(&format!("{REPO}:img")), &v("0.12.0"))).unwrap_err();
    assert!(matches!(err, FetchError::Failed(ref m) if m.contains("connection refused")));
}

// i[verify definition.fetch]
#[test]
fn a_bundle_contradicting_its_annotation_is_invalid() {
    let reg = FakeRegistry::default();
    let meta = meta(">=0.12");
    let digest = reg.push_definition(
        REPO,
        &[("app.seed.rhai", b"app;"), ("seedling.toml", &meta)],
        Some(">=0.11"),
    );
    reg.tag(REPO, "1", &digest);
    let err = block(fetch(&reg, &r(&format!("{REPO}:1")), &v("0.12.0"))).unwrap_err();
    assert!(
        matches!(err, FetchError::Bundle(BundleError::Invalid(_))),
        "{err:?}"
    );
}

fn index_of(reg: &FakeRegistry, annotations: &[Option<&str>]) -> Vec<String> {
    let digests: Vec<String> = annotations
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let body = format!("// entry {i}\n");
            let meta = a.map(meta);
            let mut files: Vec<(&str, &[u8])> = vec![("app.seed.rhai", body.as_bytes())];
            if let Some(m) = &meta {
                files.push(("seedling.toml", m));
            }
            reg.push_definition(REPO, &files, *a)
        })
        .collect();
    let entries: Vec<(&str, Option<&str>, Option<&str>)> = digests
        .iter()
        .zip(annotations)
        .map(|(d, a)| (d.as_str(), Some(ARTIFACT_TYPE), *a))
        .collect();
    let index = reg.push_index(REPO, &entries);
    reg.tag(REPO, "x", &index);
    digests
}

// i[verify definition.fetch.select]
#[test]
fn highest_minimum_candidate_is_selected() {
    let reg = FakeRegistry::default();
    let d = index_of(&reg, &[Some(">=0.12"), Some(">=0.13")]);
    let got = block(resolve_digest(&reg, &r(&format!("{REPO}:x")), &v("0.13.0"))).unwrap();
    assert_eq!(got, d[1]);

    let reg = FakeRegistry::default();
    let d = index_of(&reg, &[Some(">=0.12"), Some(">=0.14")]);
    let got = block(resolve_digest(&reg, &r(&format!("{REPO}:x")), &v("0.13.0"))).unwrap();
    assert_eq!(got, d[0]);
}

// i[verify definition.fetch.select]
#[test]
fn unannotated_entry_is_the_last_resort() {
    let reg = FakeRegistry::default();
    let d = index_of(&reg, &[None, Some(">=0.12")]);
    let got = block(resolve_digest(&reg, &r(&format!("{REPO}:x")), &v("0.13.0"))).unwrap();
    assert_eq!(got, d[1]);
    let got = block(resolve_digest(&reg, &r(&format!("{REPO}:x")), &v("0.11.0"))).unwrap();
    assert_eq!(got, d[0]);
}

// i[verify definition.fetch.select]
#[test]
fn no_candidate_or_a_tie_refuses() {
    let reg = FakeRegistry::default();
    index_of(&reg, &[Some(">=0.14")]);
    let err = block(resolve_digest(&reg, &r(&format!("{REPO}:x")), &v("0.13.0"))).unwrap_err();
    assert!(
        matches!(err, FetchError::Bundle(BundleError::Unsupported { .. })),
        "{err:?}"
    );

    let reg = FakeRegistry::default();
    let d = index_of(&reg, &[Some(">=0.12"), Some(">=0.12, <1")]);
    let err = block(resolve_digest(&reg, &r(&format!("{REPO}:x")), &v("0.13.0"))).unwrap_err();
    match err {
        FetchError::Failed(m) => assert!(m.contains(&d[0]) && m.contains(&d[1]), "{m}"),
        other => panic!("expected a tie, got {other:?}"),
    }
}

// l[verify bsl.bundle.seedling-versions]
#[test]
fn alternatives_are_candidates_when_any_matches() {
    let reg = FakeRegistry::default();
    let d = index_of(&reg, &[Some(">=0.12"), Some(">=0.14 || >=0.11, <0.12")]);
    // At 0.11.5 only the second entry is a candidate.
    let got = block(resolve_digest(&reg, &r(&format!("{REPO}:x")), &v("0.11.5"))).unwrap();
    assert_eq!(got, d[1]);
    // At 0.14, the second's minimum is 0.11 (its lowest), below the first's 0.12.
    let got = block(resolve_digest(&reg, &r(&format!("{REPO}:x")), &v("0.14.0"))).unwrap();
    assert_eq!(got, d[0]);
}

// i[verify definition.fetch.access]
#[test]
fn allowlist_gates_before_any_request() {
    let allowed = vec!["docker.io".to_owned()];
    let err = check_allowed(&allowed, &r("ghcr.io/org/app-def:1")).unwrap_err();
    assert!(matches!(err, FetchError::NotAllowed(_)));
    check_allowed(&allowed, &r("docker.io/org/app-def:1")).unwrap();
}

// i[verify definition.tags]
#[test]
fn tags_are_listed_for_a_repository() {
    let reg = FakeRegistry::default();
    let digest = reg.push_definition(REPO, &[("app.seed.rhai", b"")], None);
    reg.tag(REPO, "1", &digest);
    reg.tag(REPO, "2", &digest);
    let repo = DefinitionRef::parse_repository(REPO).unwrap();
    assert_eq!(block(list_tags(&reg, &repo)).unwrap(), vec!["1", "2"]);
    assert!(DefinitionRef::parse_repository("ghcr.io/org/app-def:1").is_err());
}

// i[verify definition.fetch]
#[test]
fn a_manifest_that_does_not_hash_to_its_digest_is_refused() {
    let other = br#"{"schemaVersion":2,"artifactType":"other","layers":[]}"#;

    // A registry claiming the digest it was asked for while serving other
    // bytes, which is what a header-reported digest lets it do.
    let reg = FakeRegistry::default();
    let digest = reg.push_definition(REPO, &[("app.seed.rhai", b"app;")], None);
    reg.tag(REPO, "1", &digest);
    reg.serve_instead(&format!("{REPO}:1"), other, Some(&digest));
    let e = block(fetch(&reg, &r("ghcr.io/org/app-def:1"), &v("0.12.0"))).unwrap_err();
    assert!(e.to_string().contains("hash to"), "{e}");

    // A pinned digest, the form an operator uses to pin content, where the
    // registry reports the bytes it actually served.
    let reg = FakeRegistry::default();
    let digest = reg.push_definition(REPO, &[("app.seed.rhai", b"app;")], None);
    reg.serve_instead(&format!("{REPO}@{digest}"), other, None);
    let e = block(fetch(&reg, &r(&format!("{REPO}@{digest}")), &v("0.12.0"))).unwrap_err();
    assert!(e.to_string().contains("hashes to"), "{e}");

    // An index entry, whose digest the index named.
    let reg = FakeRegistry::default();
    let child = reg.push_definition(REPO, &[("app.seed.rhai", b"app;")], None);
    let index = reg.push_index(REPO, &[(&child, Some(ARTIFACT_TYPE), None)]);
    reg.tag(REPO, "1", &index);
    reg.serve_instead(&format!("{REPO}@{child}"), other, None);
    let e = block(fetch(&reg, &r("ghcr.io/org/app-def:1"), &v("0.12.0"))).unwrap_err();
    assert!(e.to_string().contains("was served with digest"), "{e}");
}

// i[verify definition.fetch]
// i[verify definition.provenance]
#[test]
fn the_recorded_digest_is_taken_over_the_manifest_bytes() {
    let reg = FakeRegistry::default();
    let digest = reg.push_definition(REPO, &[("app.seed.rhai", b"app;")], None);
    reg.tag(REPO, "1", &digest);
    // A tag may legitimately move, and what is recorded is what was served.
    let moved = reg.push_definition(REPO, &[("app.seed.rhai", b"newer;")], None);
    reg.tag(REPO, "1", &moved);
    let got = block(fetch(&reg, &r("ghcr.io/org/app-def:1"), &v("0.12.0"))).unwrap();
    assert_eq!(got.digest, moved);
    assert_ne!(got.digest, digest);
}
