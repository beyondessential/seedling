use super::*;

/// Evaluate `source` in the static context.
fn eval(source: &str) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
    let (engine, mut scope, _app) = setup(&crate::ScriptLimits::default());
    engine.eval_with_scope(&mut scope, source)
}

fn parse(value: &str) -> String {
    format!("std::parse_purl({value:?})")
}

/// The map `std::parse_purl` returns for `value`.
fn components(value: &str) -> rhai::Map {
    eval(&parse(value))
        .unwrap_or_else(|e| panic!("{value} should be accepted, got {e}"))
        .try_cast::<rhai::Map>()
        .unwrap_or_else(|| panic!("{value}: expected a map"))
}

/// The components of `value`, each as a string, for a purl without a version.
fn texts(value: &str) -> Vec<(String, String)> {
    let mut texts: Vec<_> = components(value)
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    texts.sort();
    texts
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut pairs: Vec<_> = expected
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    pairs.sort();
    pairs
}

/// The reason `std::parse_purl` gives for refusing `value`.
fn refusal(value: &str) -> String {
    match *eval(&parse(value)).expect_err(value) {
        rhai::EvalAltResult::ErrorRuntime(ref reason, _) => reason.to_string(),
        other => panic!("{value}: expected a script error, got {other:?}"),
    }
}

fn truth(source: &str) -> bool {
    eval(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .as_bool()
        .unwrap_or_else(|t| panic!("{source}: expected a boolean, got {t}"))
}

// spec: PURL#components
#[test]
fn a_purl_parses_into_its_components() {
    let map = components("pkg:npm/%40scope/foo@1.2.0?arch=x64&checksum=SHA256:BB,sha1:aa#lib/util");
    assert_eq!(map.len(), 6);
    assert_eq!(map["type"].to_string(), "npm");
    assert_eq!(map["namespace"].to_string(), "@scope");
    assert_eq!(map["name"].to_string(), "foo");
    let version = map["version"]
        .clone()
        .try_cast::<crate::defs::version::Version>()
        .expect("version is a Version");
    assert_eq!(version.to_string(), "1.2.0");
    assert_eq!(map["subpath"].to_string(), "lib/util");

    let qualifiers = map["qualifiers"].clone().try_cast::<rhai::Map>().unwrap();
    assert_eq!(qualifiers.len(), 2);
    assert_eq!(qualifiers["arch"].to_string(), "x64");
    let checksums: Vec<String> = qualifiers["checksum"]
        .clone()
        .into_array()
        .expect("checksum is an array")
        .into_iter()
        .map(|entry| entry.into_string().expect("each entry is a string"))
        .collect();
    assert_eq!(checksums, ["sha1:aa", "sha256:bb"]);
}

// spec: PURL#components
#[test]
fn absent_components_are_left_out() {
    assert_eq!(
        texts("pkg:cargo/serde"),
        pairs(&[("type", "cargo"), ("name", "serde")])
    );
    assert_eq!(
        texts("pkg:npm/foo@"),
        pairs(&[("type", "npm"), ("name", "foo")])
    );
    assert_eq!(
        texts("pkg:npm/foo?arch=&os="),
        pairs(&[("type", "npm"), ("name", "foo")])
    );
    assert_eq!(
        texts("pkg:npm/foo#./..//%2E%2E"),
        pairs(&[("type", "npm"), ("name", "foo")])
    );
}

// spec: PURL#accepted-values
// spec: PURL#components
#[test]
fn lenient_input_reads_as_the_parsing_procedure_reads_it() {
    assert_eq!(
        texts("pkg://NPM/foo"),
        pairs(&[("type", "npm"), ("name", "foo")])
    );
    assert_eq!(
        texts("pkg:npm//ns//foo"),
        pairs(&[("type", "npm"), ("namespace", "ns"), ("name", "foo")])
    );
    assert_eq!(
        texts("pkg:golang/github.com/a%20b/c%2Bd#/x/./y/../z/"),
        pairs(&[
            ("type", "golang"),
            ("namespace", "github.com/a b"),
            ("name", "c+d"),
            ("subpath", "x/y/z"),
        ])
    );

    let map = components("pkg:npm/foo?arch=&bar=1&FOO=a%26b");
    let qualifiers = map["qualifiers"].clone().try_cast::<rhai::Map>().unwrap();
    assert_eq!(qualifiers.len(), 2);
    assert_eq!(qualifiers["bar"].to_string(), "1");
    assert_eq!(qualifiers["foo"].to_string(), "a&b");
}

// spec: PURL#accepted-values
// spec: PURL#components
#[test]
fn any_type_is_accepted_without_type_specific_rules() {
    assert_eq!(
        texts("pkg:pypi/Foo_Bar"),
        pairs(&[("type", "pypi"), ("name", "Foo_Bar")])
    );
    assert_eq!(
        texts("pkg:github/Foo/Bar"),
        pairs(&[("type", "github"), ("namespace", "Foo"), ("name", "Bar")])
    );
    assert_eq!(
        texts("pkg:my.own-type2/thing"),
        pairs(&[("type", "my.own-type2"), ("name", "thing")])
    );
    assert_eq!(
        texts("pkg:maven/commons-io"),
        pairs(&[("type", "maven"), ("name", "commons-io")])
    );
}

// spec: PURL#accepted-values
#[test]
fn a_purl_breaking_a_rule_is_refused_with_its_reason() {
    for (value, reason) in [
        ("npm/foo", "starts with \"pkg:\""),
        ("PKG:npm/foo", "starts with \"pkg:\""),
        ("http://example.com/foo", "starts with \"pkg:\""),
        ("pkg:a+b/foo", "type \"a+b\""),
        ("pkg:3nginx/nginx", "type is a letter"),
        ("pkg:npm", "non-empty name"),
        ("pkg:npm/@1.0", "non-empty name"),
        ("pkg:npm/foo/", "non-empty name"),
        ("pkg:npm/foo/@1.0", "non-empty name"),
        ("pkg:npm/a%2Fb/foo", "hold no `/`"),
        ("pkg:npm/foo#a%2Fb", "hold no `/`"),
        ("pkg:npm/foo%FF", "valid UTF-8"),
        ("pkg:npm/foo%0A", "name contains U+000A"),
        ("pkg:npm/foo%E2%80%A8", "name contains U+2028"),
        ("pkg:npm/a%09/foo", "namespace contains U+0009"),
        ("pkg:npm/foo#a%00", "subpath contains U+0000"),
        ("pkg:npm/foo?arch=x%0D", "qualifier arch contains U+000D"),
        ("pkg:npm/foo?FOO", "qualifier \"FOO\" has no `=`"),
        ("pkg:npm/foo?", "empty qualifier"),
        ("pkg:npm/foo?a=1&&b=2", "empty qualifier"),
        ("pkg:npm/foo?1a=x", "qualifier key \"1a\""),
        ("pkg:npm/foo?a+b=x", "qualifier key \"a+b\""),
        (
            "pkg:npm/foo?arch=x64&ARCH=arm64",
            "\"arch\" appears more than once",
        ),
        (
            "pkg:npm/foo?arch=&arch=x64",
            "\"arch\" appears more than once",
        ),
        (
            "pkg:npm/foo?arch=x64&arch=",
            "\"arch\" appears more than once",
        ),
        (
            "pkg:npm/foo?checksum=sha1:abc",
            "even number of hexadecimal digits",
        ),
        (
            "pkg:npm/foo?checksum=sha1:xy",
            "even number of hexadecimal digits",
        ),
        ("pkg:npm/foo?checksum=sha1", "`algorithm:digits` entries"),
        (
            "pkg:npm/foo?checksum=sha1:aa,,md5:bb",
            "`algorithm:digits` entries",
        ),
        (
            "pkg:npm/foo?checksum=sha1:aa,SHA1:bb",
            "an algorithm named once",
        ),
        (
            "pkg:npm/foo?checksum=:ab",
            "entry \":ab\" does not name an algorithm",
        ),
        (
            "pkg:npm/foo?checksum=sha1:",
            "entry \"sha1:\" does not name an algorithm",
        ),
        (
            "pkg:npm/foo@1%200",
            "the version \"1 0\": a version holds only printable ASCII",
        ),
        ("pkg:npm/foo@%C4%81", "the version \"ā\""),
    ] {
        let refused = refusal(value);
        assert!(refused.contains(reason), "{value}: {refused}");
    }
}

/// The parse cases of the purl specification's own test suite, at its
/// v1.1.1 release, all of which expect a failure.
// spec: PURL#accepted-values
#[test]
fn the_purl_specification_parse_cases_are_refused() {
    for value in [
        "EnterpriseLibrary.Common@6.0.1304",
        "pkg:EnterpriseLibrary.Common@6.0.1304",
        "pkg:n&g?inx/nginx@0.8.9",
        "pkg:3nginx/nginx@0.8.9",
        "pkg:nginx:a/nginx@0.8.9",
        "pkg:npm/myartifact@1.0.0?in%20production=true",
        "pkg:maven/@1.3.4",
        "pkg%3Amaven/org.apache.commons/io",
    ] {
        refusal(value);
    }
}

// spec: PURL#components
// spec: VER#generic-versions
#[test]
fn a_purl_version_is_a_version() {
    let version = format!("{}.version", parse("pkg:deb/debian/curl@7.50.3-1"));
    assert!(truth(&format!(
        "{version} == std::parse_version(\"7.50.3-1\")"
    )));
    assert!(truth(&format!("{version} < std::parse_version(\"7.51\")")));
    assert!(truth(&format!(
        "{} > {version}",
        parse("pkg:docker/library/postgres@latest") + ".version"
    )));
    assert_eq!(
        eval(&format!(
            "{}.version.to_string()",
            parse("pkg:npm/foo@v1.0%2Bb")
        ))
        .unwrap()
        .into_string()
        .unwrap(),
        "v1.0+b"
    );
}

// spec: PURL#components
#[test]
fn a_script_reads_components_as_properties() {
    let purl = parse("pkg:npm/%40scope/foo@1.2.0?arch=x64#lib");
    assert!(truth(&format!(
        "let p = {purl}; p.type == \"npm\" && p.namespace == \"@scope\" && p.name == \"foo\" \
         && p.qualifiers.arch == \"x64\" && p.subpath == \"lib\" \
         && p.version >= std::parse_version(\"1.0\")"
    )));
    assert!(truth(&format!(
        "let p = {}; !(\"version\" in p) && !(\"namespace\" in p)",
        parse("pkg:cargo/serde")
    )));
}
