---
status: draft
---

# Caddy image: add the rate-limit and WAF modules

Prerequisite for B3 (per-route rate limiting) and B8 (WAF hook): add the rate-limit and WAF modules to the prebuilt Caddy image so the running binary understands configs that reference them, then bump the pinned tag with the version-compatibility discipline the Containerfile already requires.
