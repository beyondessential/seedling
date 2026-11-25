# H7: URL and hostname parameter kinds

Adds the `url`, `url-origin` and `hostname` kinds and their `std::parse_*` helpers (URLs and hostnames spec, URL).

## Tech notes

- **Parsing builds on `url` 2.5.9** (servo/rust-url, WHATWG URL Standard, tested against the web platform tests, already in the dependency tree through reqwest and others), with `idna` 1.1.0 for `hostname` (`domain_to_ascii_strict`: UTS #46 with hyphen checks, STD3 ASCII rules and DNS length checks) and `percent-encoding` 2.3.2 for any decoding of userinfo.
- **Ruled out: `ada-url` 4.0.0**, WHATWG too but wraps a C++ library; **`fluent-uri` 0.4.1**, RFC 3986, which the WHATWG decision rules out.
- **Where `url` differs from what we want**, found by probing 2.5.8:
  - non-special schemes (`postgres`, `redis`, …) get an opaque host: case kept, Unicode percent-encoded rather than punycoded, `0x7f.1` left as a name. We hand it, undecoded, to `url::Host::parse` (the non-opaque host parser, which does its own percent-decoding), keeping the opaque host only where that fails. In the crate's public API an opaque host shows as `Host::Domain` on a URL that is not `is_special()`, which is how we tell it apart;
  - `port_or_known_default` knows only http, https, ws, wss and ftp, which we keep: a table of our own would serve little beyond `postgres`;
  - port `0` is accepted, which we refuse, as BSL's port rules do;
  - username is always a string (empty when absent), and username and password come back percent-encoded, which we decode with `percent-encoding`, refusing invalid UTF-8 and the single-line kinds' banned categories;
  - a host-less URL (`file:///x`, `foo://`) and an opaque one (`mailto:`, `data:`) both parse, which we keep: the kind is for any URL a definition might take, not only network endpoints;
  - `idna::domain_to_ascii_strict` accepts `1.2.3.4`, so `parse_hostname` adds the ends-in-a-number check, as below.
- **Fits alongside `purl`.** E7 added `ParsedValue::Purl` and `defs/purl.rs`, with the struct converted to a Rhai map in `stdlib/kinds.rs` and absent keys not inserted. `url` and `url-origin` follow the same shape. The decoded userinfo check reuses `line_breaking_character` in `param_def.rs` rather than copying it.
- **The ends-in-a-number check comes from the crate, not a copy of it.** The checker is private in `url`, but for a name that has already passed IDNA processing and the ingress rules, `url::Host::parse` returns `Host::Domain` exactly when the checker refuses it; a name ending in a number comes back as `Host::Ipv4` (`10.0.0.5`) or an error (`app.0x1f`). So `parse_hostname` accepts only a `Host::Domain` result.
- **The `@` check for `url-origin` reads the input.** The crate drops empty userinfo, so `https://@h/` and `https://h/` parse to the same `Url`. After the parse has confirmed an `http` or `https` scheme, scan the authority as the parser reads it: from the first `:`, skip any run of `/` and `\`, then up to the first `/`, `\`, `?` or `#`, and refuse if it holds `@`. Query and fragment need no scan, since `query()` and `fragment()` return `Some("")` for a bare `?` or `#`.
- **One ingress hostname check.** `defs/service.rs` and `oi/handler/ingresses.rs` each carry a `validate_hostname` for ING's hostname rules, differing only in error type. `parse_hostname` would be a third caller, so the rule moves to one function returning `Result<(), String>` and each caller wraps the message in its own error.

## Dependencies

- [ ] `cargo add -p seedling-core url@2.5.9 idna@1.1.0 percent-encoding@2.3.2`, then check `Cargo.lock` holds one copy of each

## Shared ingress hostname check

- [ ] Add `pub(crate) fn check_hostname(hostname: &str) -> Result<(), String>` to `crates/core/src/defs/ingress.rs`, carrying the body of `service.rs`'s `validate_hostname` and its `// spec: ING#hostnames`
- [ ] Remove `validate_hostname` from `crates/core/src/defs/service.rs`; the `ingress()` builder calls `check_hostname(hostname)?`, the `String` converting into `EvalAltResult`
- [ ] Remove `validate_hostname` and its doc comment from `crates/core/src/oi/handler/ingresses.rs`; the create handler maps the error to `OiError::new(ErrorCode::RequirementsInvalid, _)`
- [ ] Run the existing ingress tests in `tests/ingress.rs` and `oi/handler/ingresses/tests.rs`; the messages keep their wording, so any test asserting on them still matches

## Parsers (`crates/core/src/defs/url.rs`)

- [ ] New module `defs/url.rs` (`pub mod url;` in `defs.rs`), doc comment matching `purl.rs`'s, importing the crate as `::url` where the module name would shadow it
- [ ] `pub fn parse_hostname(text: &str) -> Result<String, String>`: `idna::domain_to_ascii_strict`, then `check_hostname`, then `url::Host::parse` must give `Host::Domain`; returns the result lower-cased, with `// spec: URL#hostnames` at each step
- [ ] `pub struct Url { scheme: String, host: Option<String>, port: Option<u16>, path: String, query: Option<String>, fragment: Option<String>, username: Option<String>, password: Option<String> }`
- [ ] `Url::parse(text)`: `::url::Url::parse`, error text from the crate's `ParseError` display
  - [ ] host: `None` for no host or an empty one; for a special URL the serialised host; for an opaque host, `Host::parse` on it as given, serialised on success, otherwise the opaque host (`// spec: URL#hosts`)
  - [ ] port: `port()` or, failing that, `port_or_known_default()`; refuse `Some(0)` (`// spec: URL#ports`)
  - [ ] query and fragment: `None` when absent or empty, otherwise as the crate gives them (`// spec: URL#components`)
  - [ ] username and password: `None` when empty, otherwise `percent_decode_str(..).decode_utf8()`, refusing invalid UTF-8 and anything `line_breaking_character` finds, naming the component and the character as `reject_control_characters` does (`// spec: URL#userinfo`)
- [ ] `pub struct UrlOrigin { scheme: String, host: String, port: u16 }` and `UrlOrigin::parse(text)`: `Url::parse` first, then in turn refuse a scheme other than `http`/`https`, an `@` in the authority (scan above), a query or fragment present at all, a path other than `/`, and a host `parse_hostname` refuses or would change; the host is what `parse_hostname` returns, the port as `Url` gave it (`// spec: URL#url-origins`)

## Kind wiring

- [ ] `crates/core/src/defs/param_def.rs`: `ParamKind::Url`, `UrlOrigin` and `Hostname` after `Purl`, each `// spec: ACN#param-kinds`; arms in `parse`, `as_str` and `FromStr` for `"url"`, `"url-origin"` and `"hostname"`. They stay single-line, so `reject_control_characters` runs before them
- [ ] `ParsedValue::Url(Url)` and `ParsedValue::UrlOrigin(UrlOrigin)`; `Hostname` returns `ParsedValue::Value` holding the lower-cased name
- [ ] `crates/core/src/defs/stdlib/kinds.rs`: `url_map` and `url_origin_map` beside `purl_map`, inserting only the keys present, `port` as an `INT`, each `// spec: URL#components` / `// spec: URL#url-origins`
- [ ] Confirm `is_secret` stays false for the three kinds (no change to its `matches!`)

## Tests

- [ ] `crates/core/src/tests/url.rs` (registered in `tests.rs`), set up like `tests/purl.rs`, each test citing its URL section:
  - [ ] hostnames: `Example.ORG` → `example.org`; `bücher.de` → `xn--bcher-kva.de`; refused: `10.0.0.5`, `app.0x1f`, `[::1]`, `*.example.org`, `-a.example.org`, a 64-character label, an empty value, `a..b`
  - [ ] urls, components: `https://u:p@Example.org:8443/a/b?x=1&x=2#f` gives every key; `https://example.org` gives port 443 and path `/`; `file:///etc/hosts` has no `host`; `mailto:someone@example.org` has scheme and path only; `https://h/?#` has no query or fragment; `%2F` in the path, query and fragment stays encoded
  - [ ] urls, hosts: `postgres://DB.Example.org/x` → host `db.example.org`; `postgres://0x7f.1/` → `127.0.0.1`; `postgres://bücher.de/` → `xn--bcher-kva.de`; `https://[::1]:80/` → `[::1]`; a non-special host the host parser refuses (`foo://a%zz/`) stays as given; `postgres://h/` has no port
  - [ ] urls, userinfo: `%40` and `%3A` in a password come back as `@` and `:`; `%0A` in a username, `%E2%80%A8` in a password and `%FF` refused
  - [ ] urls, ports: `https://h:0/` and `postgres://h:0/` refused; an unparseable URL (`not a url`, `https://h:99999/`) refused
  - [ ] url origins: `https://Example.org`, `http://example.org:8080/` accepted with the expected map; refused: `ftp://example.org/`, `https://@example.org/`, `https://u@example.org/`, `https://example.org/?`, `https://example.org/#`, `https://example.org/a`, `https://10.0.0.5/`, `https://[::1]/`, `https://app.0x1f/`, `https://h:0/`
  - [ ] `std::parse_url` on a value with U+2028 is refused before the URL parser runs
- [ ] `crates/core/src/tests/stdlib.rs`: add `parse_url`, `parse_url_origin` and `parse_hostname` to the single-line refusal list
- [ ] `crates/core/src/tests/param.rs`: `param_schema_kind_url` covering all three kinds through `.kind(..)` and an action param schema, asserting the kind and that none is secret
- [ ] `crates/web/frontend/src/components/ParamFields.test.tsx`: add `url`, `url-origin` and `hostname` to the single-line input test and retitle it

## Docs and checks

- [ ] `docs/bsl-scripting.md`: add the kinds to the `.kind(..)` list (line 171) and the parsers to the list at line 395; a subsection after the purl one on what `std::parse_url`, `std::parse_url_origin` and `std::parse_hostname` return, with an example and the advice to mark a URL carrying a password `secret(true)`
- [ ] `cargo clippy --all-targets`, `cargo fmt`, `etc/ci/check-spec-style.sh`, `etc/ci/check-spec-citations.sh`, the frontend tests for `ParamFields`
