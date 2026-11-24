---
id: PURL
---

# Package URLs

Applies to the `purl` [param kind](action.md#param-kinds), its [parser](action.md#kind-parsers), and the map that parser returns.

## Borrowed terms

| term | meaning |
| --- | --- |
| Package URL, purl | as defined by [ECMA-427, 2nd edition](https://ecma-tc54.github.io/ECMA-427/) |
| type, namespace, name, version, qualifiers, subpath | the components of a Package URL, as ECMA-427 defines them |
| lenient PURL grammar | the [lenient grammar](https://github.com/package-url/purl-spec/blob/v1.1.1/docs/specification/lenient-ABNF-grammar.md) the purl specification gives, at its v1.1.1 release, for the input a parser attempts |
| parsing procedure | the purl specification's [How to parse a PURL](https://github.com/package-url/purl-spec/blob/v1.1.1/docs/specification/how-to-parse.md), at its v1.1.1 release |

## Accepted values

`std::parse_purl(text)` MUST return an object map for a value it accepts.
It MUST accept a value that matches the lenient PURL grammar, with its scheme in any case, and that the parsing procedure splits into components without error.
It MUST accept a purl of any type, whether or not the purl specification defines that type.

> [!NOTE]
> Operators paste purls from SBOMs and other tools, which often write them in a form other than the canonical one, such as `pkg://npm/foo` or a type in upper case.
> The parameter's value stays as the operator entered it, so a workload receiving it gets the operator's text.

`std::parse_purl` MUST also reject a value:

- whose name is empty once percent-decoded;
- in which any percent-decoded component is not valid UTF-8;
- in which any percent-decoded component holds a character whose Unicode general category is Control (Cc), Line Separator (Zl) or Paragraph Separator (Zp);
- holding a namespace or subpath segment that contains a `/` once percent-decoded;
- holding two qualifiers with non-empty values whose keys are equal once in lower case;
- holding a non-empty version that, once percent-decoded, [`std::parse_version`](version.md#generic-versions) rejects.

> [!NOTE]
> The decoded components are what a script passes on to a workload, so a line break encoded as `%0A` carries the same risk the [single-line rule](action.md#kind-parsers) guards the raw text against.
> Keeping one of two repeated qualifiers would drop the other, and which one the workload's own tooling would have kept is unknown.

> [!NOTE]
> A script that takes only some types of package checks the `type` in a [validator](parameter.md#validators).

## Components

| key | present | value |
| --- | --- | --- |
| `type` | always | the type, in lower case |
| `namespace` | when the purl has a namespace | the percent-decoded namespace segments, without empty ones, joined by `/` |
| `name` | always | the percent-decoded name |
| `version` | when the purl has a non-empty version | a `Version`, as [generic versions](version.md#generic-versions) specify, whose text is the percent-decoded version |
| `qualifiers` | when the purl has a qualifier with a non-empty value | an object map from each such qualifier's key, in lower case, to its percent-decoded value |
| `subpath` | when the purl has a subpath segment other than an empty one, `.` or `..` | the percent-decoded subpath segments, without empty ones, `.` or `..`, joined by `/` |

`std::parse_purl` MUST return every component as the parsing procedure gives it, without the procedure's type-specific normalisation steps.

The value of a `checksum` qualifier MUST be an array of the strings its percent-decoded value holds between `,` separators, in order.
Every other qualifier's value MUST be a string.

> [!NOTE]
> A purl's `version` orders as a generic version, which agrees with most types' own schemes on ordinary releases but not on every value.
