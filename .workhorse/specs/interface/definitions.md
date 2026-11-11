---
id: DEFN
---

# App definitions

## App Definitions

<!-- from interface: App Definitions -->

> i[definition.source]
> A request that supplies an app definition carries exactly one of:
>
> - `script`: BSL source text, taken as a [bundle](rule:l/bsl.bundle) holding only `app.seed.rhai`.
> - `bundle`: an object map from bundle path to base64-encoded file contents.
> - `reference`: a complete OCI reference, from which the runtime [fetches](rule:i/definition.fetch) the bundle.
>
> A request carrying none of them, or more than one, is rejected with `requirements_invalid`.
> Alongside `script` or `bundle`, an optional `origin` object with string fields `url` and `revision` describes where the client obtained the files.
> The runtime records it in the definition's [provenance](rule:i/definition.provenance) as reported by the client, without verifying it.

> i[definition.bundle.limits]
> A bundle, pushed or fetched, is rejected with `bundle_invalid`, and nothing is stored, when:
>
> - its total size exceeds the runtime's bundle size limit, or it holds more files than the runtime's bundle file limit;
> - it holds anything other than regular files and the directories containing them;
> - a path in it is absolute, contains a null byte, or escapes the bundle root after normalisation;
> - its [metadata file](rule:l/bsl.bundle.metadata) cannot be parsed, has a field of the wrong type, or has a field it does not define;
> - its list of script files is empty or names a file twice, or a script file it names is missing from the bundle or is not valid UTF-8.
>
> The message names the offending path or field.

> i[definition.bundle.seedling-versions]
> Installing a definition into an app refuses it with `unsupported_seedling`, naming the requirement and the running version, when the running Seedling does not satisfy the definition's declared [Seedling version requirement](rule:l/bsl.bundle.seedling-versions); nothing is stored.
> The requirement is checked before any other part of the metadata, so a bundle written for a newer Seedling is reported as unsupported rather than as malformed.

> i[definition.content-hash]
> Every bundle is identified by a content hash computed over its paths and file contents alone.
> The same files yield the same hash whether they were pushed or fetched, and in whatever order or archive they arrived.

> i[definition.fetch]
> Fetching resolves `reference` against its registry and takes the definition artefact from what it names:
>
> - a manifest whose artifact type is `application/vnd.bes.seedling.definition.v1`, which is the artefact; or
> - an image index, from whose entries of that artifact type one is [selected](rule:i/definition.fetch.select). The index's other entries, such as container images, play no part.
>
> A definition artefact has a single layer, of media type `application/vnd.bes.seedling.definition.v1.tar+gzip`: a gzip-compressed tar archive whose root is the bundle root.
> Every manifest read during a fetch is accepted only when its bytes hash to the digest it was requested under: the digest a reference names, the digest an index entry gives, and the digest the registry reports all have to agree with the bytes served.
> The digest the fetch records as [provenance](rule:i/definition.provenance) is the one taken over those bytes, so it names the definition that was installed rather than what the registry asserted.
> A definition manifest may carry the annotation `vnd.bes.seedling.versions`, holding the Seedling version requirement its bundle's metadata declares, and an index entry may carry the same annotation on its descriptor.
> A fetched bundle whose declared requirement differs from its manifest's annotation is rejected with `bundle_invalid`.
> Any failure to fetch, whether the registry is unreachable, the reference is not found, credentials are refused, or the reference names no definition artefact, refuses the request with `fetch_failed`, the message giving the cause, and nothing is stored.

> i[definition.fetch.select]
> Among an image index's definition entries, the candidates are those whose `vnd.bes.seedling.versions` annotation the running Seedling satisfies; an entry without the annotation is a candidate for every version.
> The candidate whose requirement has the highest minimum is selected, the minimum of a requirement being the lowest version that satisfies it, and an entry without the annotation having the lowest minimum of all.
> The fetch is refused with `unsupported_seedling` when there is no candidate, and with `fetch_failed`, naming the tied entries, when more than one candidate shares the highest minimum.

> i[definition.fetch.access]
> A fetch is refused with `registry_not_allowed` when the reference's registry is not on the [registry allowlist](rule:i/registry.list), before any request is made to that registry.
> Fetching authenticates with the same registry credentials the host's container engine uses to pull images, so a registry the host can pull images from needs no further configuration to serve definitions.

> i[definition.provenance]
> Every app definition carries its *provenance*, reported as an object with a `kind` field and the fields for that kind:
>
> - `"fetched"`: `reference`, the reference as given, and `digest`, the digest of the definition manifest that was selected.
> - `"pushed"`: `pushed_by`, the [actor](rule:i/wire.actor) that supplied the bundle, and `reported_origin`, the `origin` the client reported or `null` when it reported none.
>
> Both kinds also carry `content_hash`, the bundle's [content hash](rule:i/definition.content-hash), and `seedling_versions`, the bundle's declared Seedling version requirement or `null` when it declares none.

> i[definition.tags]
> `/registries/tags { repository }` lists the tags of an OCI repository, named as a reference without tag or digest, and returns `{ tags }`, an array of strings.
> It is refused with `registry_not_allowed` or `fetch_failed` on the same terms as a [fetch](rule:i/definition.fetch.access).
