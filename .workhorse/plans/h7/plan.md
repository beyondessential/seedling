# H7: URL and hostname parameter kinds

Adds the `url`, `url-origin` and `hostname` kinds and their `std::parse_*` helpers (URLs and hostnames spec, URL).

## Tech notes

- **Parsing builds on `url` 2.5.9** (servo/rust-url, WHATWG URL Standard, tested against the web platform tests, already in the dependency tree through reqwest and others), with `idna` 1.1.0 for `hostname` (`domain_to_ascii_strict`: UTS #46 with hyphen checks, STD3 ASCII rules and DNS length checks) and `percent-encoding` 2.3.2 for any decoding of userinfo.
- **Ruled out: `ada-url` 4.0.0**, WHATWG too but wraps a C++ library; **`fluent-uri` 0.4.1**, RFC 3986, which the WHATWG decision rules out.
- **Where `url` differs from what we want**, found by probing 2.5.8:
  - non-special schemes (`postgres`, `redis`, …) get an opaque host: case kept, Unicode percent-encoded rather than punycoded, `0x7f.1` left as a name. We percent-decode it and run it through `url::Host::parse` (the non-opaque host parser), keeping the opaque host only where that fails;
  - `port_or_known_default` knows only http, https, ws, wss and ftp, which we keep: a table of our own would serve little beyond `postgres`;
  - port `0` is accepted, which we refuse, as BSL's port rules do;
  - username is always a string (empty when absent), and username and password come back percent-encoded, which we decode with `percent-encoding`, refusing invalid UTF-8 and the single-line kinds' banned categories;
  - a host-less URL (`file:///x`, `foo://`) and an opaque one (`mailto:`, `data:`) both parse, which we keep: the kind is for any URL a definition might take, not only network endpoints;
  - `idna::domain_to_ascii_strict` accepts `1.2.3.4`, so the all-digit last label check is ours.
