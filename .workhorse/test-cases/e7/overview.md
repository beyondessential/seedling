# E7: Package URL parameter kind

## Versions

- [x] `latest`, `main`, `v` and a digest parse as versions; the empty string is refused (verifies spec: VER)
- [x] Every numbered version sorts below every non-numbered one, and `ver2.0` counts as non-numbered (verifies spec: VER)
- [x] Non-numbered versions compare by their bytes, so `Latest` sorts below `latest` and the two are unequal (verifies spec: VER)
- [x] An oversized digit run is a string segment in a digest and refused in a numbered version, with or without a leading `v` (verifies spec: VER)
- [x] A version param and a stored version parameter refuse a value containing a space (verifies spec: VER)

## Package URLs

- [x] A full purl gives every component, with `version` a `Version` and `checksum` a sorted, lower-cased array (verifies spec: PURL)
- [x] Absent components are left out: no namespace, an empty version, all-empty qualifiers, a subpath of only `.`, `..` and empty segments (verifies spec: PURL)
- [x] Lenient input reads as the parsing procedure reads it: leading slashes, an upper-case type, doubled slashes in the namespace, upper-case qualifier keys (verifies spec: PURL)
- [x] Any type is accepted and no type-specific rules apply (`pypi`, `github`, `maven` without a namespace, an unknown type) (verifies spec: PURL)
- [x] Each rejection rule refuses with its reason: scheme, type characters, empty name, trailing `/`, decoded `/`, invalid UTF-8, decoded control characters in each component, bare and empty qualifiers, bad keys, repeated keys in either order, bad checksums, a version `parse_version` refuses (verifies spec: PURL)
- [x] The purl specification's v1.1.1 parse cases are all refused (verifies spec: PURL)
- [x] A script reads the components as map properties, `type` included, and compares `version` with `std::parse_version` (verifies spec: PURL)
- [x] `purl` is accepted as a static and an action param kind, and a raw control character is refused like any single-line kind (verifies spec: ACN)
- [x] A stored `purl` parameter is refused with the parser's reason, and an accepted one is stored as written (verifies spec: PURL)
- [x] The web form offers a single-line input for a `purl` param (verifies spec: ACN)
