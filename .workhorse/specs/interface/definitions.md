---
id: DEFN
---

# App definitions

Applies to how a request supplies an app's definition, how the [server](overview.md#server) checks or fetches it, and how the definition's origin is reported.

## Borrowed terms

| term | meaning |
| --- | --- |
| manifest, image index, descriptor, artifact type, annotation | as defined in the [OCI Image Format Specification](https://github.com/opencontainers/image-spec) |
| registry, repository, tag, reference | as defined in the [OCI Distribution Specification](https://github.com/opencontainers/distribution-spec) |
| digest | a content identifier of the form `sha256:` followed by 64 lowercase hexadecimal digits, as the OCI Image Format Specification defines it |
| base64 | the encoding defined in [RFC 4648](https://www.rfc-editor.org/rfc/rfc4648) section 4, with padding |
| gzip | the format defined in [RFC 1952](https://www.rfc-editor.org/rfc/rfc1952) |
| SHA-256 | the hash function defined in [FIPS 180-4](https://csrc.nist.gov/pubs/fips/180-4/upd1/final) |

## Terminology

### Content hash

A digest identifying a [definition bundle](../overview.md#definition-bundle) by its bundle paths and file contents alone.

### Provenance

The record of how an app's definition reached the host.

### Definition artefact

An OCI manifest that holds a definition bundle as its single layer.

## Definition sources

A request that supplies a definition MUST carry exactly one of these members.

| member | type | meaning |
| --- | --- | --- |
| `script` | string | the text of a script, taken as a bundle holding only that script, as [bundle paths](../language/bundle.md#bundle-paths) specifies |
| `bundle` | object | the bundle's files, each member's name a [bundle path](../language/bundle.md#bundle-path) and its value the file's contents in base64 |
| `reference` | string | an OCI reference from which the server [fetches](#fetching) the bundle |

The server MUST refuse a request that carries none of them, or more than one, with `requirements_invalid`.

A request that carries `script` or `bundle` MAY also carry `origin`, an object with string members `url` and `revision` and no others, saying where the client obtained the files.
The server MUST record `origin` in the definition's [provenance](#provenance-reporting) as the client reported it, without verifying it.
The server MUST refuse `origin` alongside `reference`, with `requirements_invalid`.

A `reference` MUST name its registry host explicitly, and MUST name a tag, a digest, or both.
A registry host is the reference's first path component, and is explicit when it contains a `.` or a `:`, or is `localhost`.
The server MUST refuse any other reference with `requirements_invalid`, before making any request to a registry.

## Bundle rules

The server MUST refuse a bundle, pushed or fetched, with `bundle_invalid`, storing nothing, when:

- the sum of its bundle paths' lengths and its files' sizes exceeds 2,097,152 bytes;
- it holds more than 4,096 files;
- it holds anything other than regular files and the directories containing them;
- a bundle path in it is invalid, names the bundle root, or appears twice once normalised;
- one of its files lies at a bundle path beneath another of its files;
- a `bundle` member's value is not a base64 string;
- its [metadata file](../language/bundle.md#metadata-file) cannot be parsed, has a field of the wrong type, or has a field the metadata file does not define;
- its list of script files breaks the metadata file's rules.

The error's `message` MUST name the offending path or field.

The size and file-count limits govern a definition when a request supplies it.
The server MUST NOT refuse a stored definition, when it loads it again, for exceeding them.

> [!NOTE]
> The size limit leaves a bundle's base64 encoding inside the 4 MiB limit on a request or a response.

## Seedling version support

When a request would install a definition into an app, the server MUST refuse the definition with `unsupported_seedling` if the running Seedling version does not satisfy the definition's [Seedling version requirement](../language/bundle.md#seedling-version-requirement), and MUST store nothing.
The error's `message` MUST name the requirement and the running version.

The server MUST check the version requirement before any other part of the metadata, so that a bundle written for a later Seedling is reported as unsupported rather than as malformed.

## Content hash computation

A bundle's content hash MUST be `sha256:` followed by the lowercase hexadecimal SHA-256 digest of the bundle's canonical encoding.
The canonical encoding is the 19 bytes `seedling-bundle-v1` followed by a zero byte, then, for each file in ascending byte order of its normalised bundle path:

1. the length of the path in bytes, as an unsigned 64-bit integer in network byte order;
2. the path, in UTF-8;
3. the length of the file's contents in bytes, as an unsigned 64-bit integer in network byte order;
4. the file's contents.

The same files MUST yield the same content hash whether they were pushed or fetched, and in whatever order or archive they arrived.

## Fetching

To fetch a definition, the server MUST resolve the `reference` against its registry and take the definition artefact from the document it names:

- if the document is a manifest, it MUST be a definition artefact;
- if the document is an image index, the definition artefact is the entry [selected](#artefact-selection) from it, and the index's other entries play no part.

A manifest is a definition artefact if its `artifactType` is `application/vnd.bes.seedling.definition.v1`, or it has no `artifactType` and its config descriptor's `mediaType` is that value.
A definition artefact MUST have exactly one layer, of media type `application/vnd.bes.seedling.definition.v1.tar+gzip`, holding a gzip-compressed tar archive whose root is the bundle root.

The server MUST accept a manifest or index only if the SHA-256 digest of its bytes equals the digest it was requested under: the digest the reference names, the digest the index entry gives, and the digest the registry reports.
The server MUST accept the layer only if its bytes match the layer descriptor's digest, and MUST refuse a layer digest that does not use SHA-256.
The digest the server records in the provenance MUST be the one it computed over the manifest's bytes.

A definition manifest MAY carry the annotation `vnd.bes.seedling.versions`, holding the Seedling version requirement its bundle's metadata declares, and an index entry MAY carry the same annotation on its descriptor.
The server MUST refuse a fetched bundle with `bundle_invalid` if the text of its declared requirement differs from either annotation's text, or declares none where an annotation is present.

The server MUST refuse the request with `fetch_failed`, with a `message` giving the cause, and store nothing, on any failure to fetch: an unreachable registry, a reference not found, refused credentials, a digest mismatch, or a document that holds no definition artefact.

### Artefact selection

The candidates among an image index's entries are those whose `artifactType` is the definition artefact type and whose `vnd.bes.seedling.versions` annotation the running Seedling satisfies.
An entry without the annotation is a candidate for every version.

The server MUST select the candidate whose requirement has the highest minimum.
The minimum of a requirement is the lowest version that satisfies it, and an entry without the annotation has a lower minimum than any annotated entry.

The server MUST refuse the fetch with `fetch_failed` if the index holds no definition entry.
It MUST refuse it with `unsupported_seedling` if no entry is a candidate.
It MUST refuse it with `fetch_failed`, naming the tied entries, if more than one candidate shares the highest minimum.

### Registry access

The server MUST refuse a fetch with `registry_not_allowed` if the reference's registry is not on the [registry allowlist](images.md#registry-allowlist), before making any request to that registry.

The server MUST authenticate to a registry with the credentials the host's container engine uses to pull images, so that a registry the host can pull images from serves definitions without further configuration.

## Provenance reporting

The server MUST store each definition's provenance with it, and MUST report it as an object whose `kind` member selects the other members.

| kind | member | type | meaning |
| --- | --- | --- | --- |
| `"fetched"` | `reference` | string | the reference as the request gave it |
| `"fetched"` | `digest` | string | the digest of the definition manifest selected |
| `"pushed"` | `pushed_by` | object or null | the [actor](wire.md#actor) that supplied the bundle, or `null` where none was recorded |
| `"pushed"` | `reported_origin` | object or null | the `origin` the client reported, or `null` where it reported none |
| either | `content_hash` | string | the bundle's [content hash](#content-hash-computation) |
| either | `seedling_versions` | string or null | the bundle's declared Seedling version requirement, or `null` where it declares none |

## Repository tags

`/registries/tags` lists the tags of an OCI repository.

| parameter | type | required | meaning |
| --- | --- | --- | --- |
| `repository` | string | yes | the repository, named as a reference without a tag or digest |

The result is `{ "tags": [<string>, ...] }`.

The server MUST refuse a `repository` that names a tag or digest, or does not name its registry host explicitly, with `requirements_invalid`.
It MUST refuse the request with `registry_not_allowed` or `fetch_failed` on the same terms as a fetch.
