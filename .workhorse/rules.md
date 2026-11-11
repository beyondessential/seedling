# Spec and plan rules

Specs live in `.workhorse/specs/<area>/<name>.md`; plans live in `.workhorse/plans/`.

For the frontmatter `id`, the fold/create/split decision, and when a change warrants a spec update, follow `.agents/docs/spec-format.md` and the Workhorse skills.
This file records the house conventions that sit on top of that, and wins where the two disagree.

A spec is the durable description of **what** the system requires.
It is read by someone deciding whether the implementation is correct, or re-implementing the feature from scratch, not as a narrative of how the code works or how it came to be.

## House style

Specs are written in the standards voice, with requirement keywords as defined in BCP 14 and stated once in `.workhorse/specs/overview.md`.
This replaces the plain-bullet, keyword-free criteria shown in `spec-format.md`.

- Markdown prose, each sentence on its own line, no hard wrapping; related sentences form a paragraph.
- One requirement per sentence, with a named actor: "The runtime MUST …", "A client SHOULD …".
- MUST for interoperability, security properties and product commitments; SHOULD or MAY otherwise.
- Under the H1, one sentence on what the requirements apply to.
- Headings are noun phrases. A heading is also the anchor code cites, so reword one only with its citations.
- Field and message shapes go in tables, followed by the sentences the table cannot carry.
- Non-normative material goes in `> [!NOTE]` blocks, which never contain a requirement keyword.
- Terms used across specs are defined in the root spec's `## Terminology`; terms one spec owns are defined in its own `## Terminology`; terms from an external standard go in a `## Borrowed terms` table.
- No em-dashes.

## Cross-references

Specs reference each other with relative markdown links, deep-linking a heading anchor where one section is meant.
Code cites the spec it implements with an inline `// spec: ID` or `// spec: ID#section-slug` comment.
`etc/ci/check-spec-citations.sh` fails on a citation naming no spec or heading, and the Specs workflow checks every link and anchor.

## What, not how

- Describe **what** the system requires, not **how** the code achieves it.
  Keep out of spec text: tool and command names, crate and library names, syscall names, internal API details, data-structure choices, and environment variable names used only by the implementation.
- Acceptable, because external actors or other components depend on them: interface contracts such as config file paths and formats, on-disk and on-the-wire shapes, BSL signatures, CLI-visible names.
- The test: would someone re-implementing the feature from scratch be constrained to the same choice?
  If not, it is an implementation detail and does not belong in the spec.
  Wherever a contract exists, the spec carries enough for someone to write a compatible implementation from scratch.

## Present, not past

- State what the system does, not how it got there.
  No "this supersedes X", "formerly Y", changelog entries, or migration narration.
- When something is removed or replaced, edit the spec to describe the new reality and delete the old text.
  Version control is the record of change; the spec is the record of the present.

## Own behaviour, not a dependency's internals

- Describe the system's own behaviour and contracts, not a dependency's decision logic or version-specific quirks beyond the minimum needed to justify a requirement.
- No plan scaffolding in spec prose: no "Strategy A/B", "Phase N", or card tags.
