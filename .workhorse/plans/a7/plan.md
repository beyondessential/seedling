# A7: Version and SemVer parameter kinds

## Explicitly rejected

- A single SemVer-only `version` kind: `version` is generic, and SemVer is the `version-semver` opt-in.
- Requirements or ranges on a generic `Version`, including VERS range strings: the Seedling version requirement on `SemVer` is the only requirement syntax.
- Partial versions (`2.71`) as `version-semver` values.
- A `version-requirement` kind: requirements come from scripts, not operators.
