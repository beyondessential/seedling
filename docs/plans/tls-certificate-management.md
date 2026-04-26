# TLS certificate management — operator surface

## Goal

Give operators control over how TLS certificates are obtained for ingress
hostnames, and visibility into their state. Three issuance modes:

1. **Caddy ACME with DNS-01 challenge** — for hostnames that can't or shouldn't
   be reachable on :80 (private ingresses, wildcard certs, multi-IP setups).
   Route 53 first; provider list is extensible.
2. **Manual cert + key upload** — operator brings a PEM cert chain and private
   key obtained out of band.
3. **CSR flow** — server generates a keypair, hands the operator a CSR to take
   to whatever CA they use, and accepts the signed cert back later. The private
   key never leaves the server.

Caddy's default HTTP-01 ACME continues to be the fallback for any hostname
without an explicit policy, exactly as today.

Per-app visibility: every ingress's certificate state (mode, issuer,
notBefore/notAfter, status) shows up in app detail and on a dedicated
Certificates page.

## Architectural decisions

### TLS issuance strategy lives outside BSL

BSL ingresses already declare `.tls()` / `.http()` / `.http2()` — meaning
"this ingress wants TLS for hostname X". They do **not** declare *how* the
cert is obtained. That's a platform-operator concern, not an
app-developer concern: the same script must run on a node where
hostname X uses public Let's Encrypt and on a node where it uses an
internal CA via uploaded cert.

Cert strategy is therefore stored in a separate operator-controlled
table keyed by hostname, applied at Caddy-config-build time.

### Strategy is per-hostname, not per-ingress

Multiple ingresses may share a hostname (uncommon in practice but
allowed). Caddy's TLS automation is per-subject anyway, so hostname is
the natural key.

### Caddy is the cert store; we are the policy store

For ACME (HTTP-01 and DNS-01), Caddy continues to acquire, persist, and
renew certs in its own data volume. We don't try to inspect or copy
those certs.

For manual/CSR certs we do hold the cert+key (encrypted, in our DB) and
inject them into Caddy's config via `tls.certificates.load_pem`. Caddy
reads them on each config reload and serves them directly — no renewal
side, no ACME involved.

### Reuse existing secret cipher

Manual private keys and DNS-provider credentials are encrypted at rest
with the existing `runtime::secrets::Cipher` (orion AEAD, key file at
`secret_key`). Same pattern as `secret_params`.

## Spec changes (`docs/spec/runtime.md`)

New section after the existing ingress-cert observation rules:

- `r[tls.strategy.default]` — when no operator policy exists for a
  hostname declared by a TLS-terminating ingress, the runtime must
  request a public ACME cert via the HTTP-01 challenge (current
  behaviour).
- `r[tls.strategy.acme-dns]` — operators may bind a hostname to a
  configured DNS provider; the runtime must drive ACME with the
  corresponding DNS-01 challenge. Provider credentials must be stored
  encrypted.
- `r[tls.strategy.manual]` — operators may upload a PEM cert chain and
  private key for a hostname; the runtime must cause the proxy to serve
  that exact pair. Key material must be stored encrypted.
- `r[tls.csr.flow]` — operators may instruct the runtime to generate a
  keypair and CSR for a hostname; the runtime must emit a downloadable
  CSR and accept a matching signed cert later, transitioning the
  hostname to the manual strategy on upload. The generated private key
  must remain on the server, encrypted at rest, and never be exposed
  via the operator interface.
- `r[tls.cert.metadata]` — for every hostname with a TLS-terminating
  ingress, the runtime must surface to the operator interface: the
  active strategy, issuer DN, notBefore, notAfter, and acquisition
  status.
- `r[tls.fault.expiring]` — for manual/CSR-derived certs (which the
  runtime cannot auto-renew), if `notAfter` is within 14 days, the
  runtime must file a `cert_expiring_soon` fault against the affected
  ingress. ACME-issued certs are renewed by Caddy and are exempt.
- `r[tls.dns-provider.lifecycle]` — DNS provider credentials must be
  separately addressable (named) so a single credential set can serve
  multiple hostnames; deleting a provider must be refused while
  hostnames reference it.

## Database schema (migration `v42.sql`)

```sql
-- r[impl tls.dns-provider.lifecycle]
CREATE TABLE IF NOT EXISTS tls_dns_providers (
    name              TEXT PRIMARY KEY,
    kind              TEXT NOT NULL,            -- 'route53'
    config_ciphertext BLOB NOT NULL,            -- provider-specific JSON
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL
);

-- r[impl tls.csr.flow]
-- r[impl tls.strategy.manual]
-- One row per (hostname, attempt). When a CSR is signed and uploaded the
-- same row transitions; superseded rows are kept for history.
CREATE TABLE IF NOT EXISTS tls_certificates (
    id              INTEGER PRIMARY KEY,
    hostname        TEXT NOT NULL,
    state           TEXT NOT NULL,
    -- 'csr_pending'   : keypair generated, awaiting cert upload
    -- 'active'        : signed cert in service
    -- 'superseded'    : replaced by a newer cert for the same hostname
    -- 'failed'        : rejected on upload (validation error)
    cert_pem        TEXT,                        -- PEM chain (active/superseded)
    csr_pem         TEXT,                        -- PEM CSR (csr_pending)
    key_ciphertext  BLOB NOT NULL,               -- encrypted PKCS#8 private key
    issuer          TEXT,                        -- parsed out at upload time
    not_before      INTEGER,
    not_after       INTEGER,
    serial          TEXT,
    note            TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS tls_certificates_hostname_state
    ON tls_certificates (hostname, state);

-- r[impl tls.strategy.acme-dns]
-- r[impl tls.strategy.manual]
-- The active policy per hostname. Hostnames with no row use ACME HTTP-01.
CREATE TABLE IF NOT EXISTS tls_policies (
    hostname     TEXT PRIMARY KEY,
    strategy     TEXT NOT NULL,                  -- 'acme_dns' | 'manual'
    dns_provider TEXT REFERENCES tls_dns_providers(name) ON DELETE RESTRICT,
    cert_id      INTEGER REFERENCES tls_certificates(id) ON DELETE RESTRICT,
    updated_at   INTEGER NOT NULL,
    CHECK (
        (strategy = 'acme_dns' AND dns_provider IS NOT NULL AND cert_id IS NULL) OR
        (strategy = 'manual'   AND cert_id      IS NOT NULL AND dns_provider IS NULL)
    )
);
```

## Caddy build

`Containerfile.caddy` and the embedded `CADDY_CONTAINERFILE` constant in
`crates/core/src/system/caddy/startup.rs` both add:

```
--with github.com/caddy-dns/route53
```

Caddy is rebuilt the next time the daemon notices the image is missing
or outdated. The blue/green upgrade path already handles the rollover.

## Caddy config changes

`crates/core/src/system/caddy/config.rs` `build_caddy_config` gains a
`tls_policies` parameter and:

- Builds **multiple** automation policies grouped by strategy:
  - One policy per DNS provider (subjects = hostnames using that
    provider, issuer module set with the provider's config).
  - One default policy (subjects = everything else with `tls_acme`).
- Adds `tls.certificates.load_pem` entries for each manual cert.
- For `manual`-strategy hostnames, omits them from the automation
  policies (Caddy will match the loaded cert by SNI).

Provider config secrets are decrypted just before the JSON is rendered.
The cached `caddy_proxy_config` row continues to hold the full rendered
JSON, so credentials end up at rest in two places (the encrypted DNS
provider table and the cached JSON blob). Acceptable: the cache is
node-local and already at the same trust level as the daemon's running
process. Documented as such.

## Operator interface (new `oi/handler/tls.rs`)

DNS providers:
- `/tls/dns-providers/list` — `[{ name, kind, created_at, updated_at }]`
- `/tls/dns-providers/upsert` — `{ name, kind, config }`
- `/tls/dns-providers/delete` — `{ name }`

Certificates:
- `/tls/certificates/list` — `[{ id, hostname, state, issuer,
  not_before, not_after, serial, note, created_at }]`
- `/tls/certificates/upload-manual` — `{ hostname, cert_pem, key_pem,
  note? }` → `{ id }`. Validates that key matches cert.
- `/tls/certificates/csr/begin` — `{ hostname, key_type:
  'ecdsa_p256'|'rsa_2048'|'rsa_4096' }` → `{ id, csr_pem }`
- `/tls/certificates/csr/get` — `{ id }` → `{ csr_pem }` (re-download)
- `/tls/certificates/csr/upload-cert` — `{ id, cert_pem }` → `{}`.
  Validates cert public key matches stored key, transitions to
  `active`, supersedes any prior active cert for the same hostname.
- `/tls/certificates/csr/cancel` — `{ id }` → drops a pending CSR.
- `/tls/certificates/delete` — `{ id }`. Refused if referenced by a
  policy.

Policies:
- `/tls/policies/list` — joins ingress hostnames (from current AppDefs)
  with policy rows + observed Caddy cert state.
  Returns `[{ hostname, app, strategy, dns_provider?, cert_id?,
  issuer?, not_before?, not_after?, status }]`.
- `/tls/policies/set` — `{ hostname, strategy, dns_provider?,
  cert_id? }`. Setting `manual` with `cert_id` switches the bound cert.
- `/tls/policies/clear` — `{ hostname }` → reverts to default
  HTTP-01.

## CLI (new `crates/ctl/src/tls.rs`)

```
seedling-ctl tls
  dns-providers
    list
    set <name> --kind route53 --config <json|@file>
    delete <name>
  certs
    list
    upload-manual <hostname> --cert <pem-file> --key <pem-file> [--note ...]
    csr begin <hostname> [--key-type ecdsa-p256|rsa-2048|rsa-4096]
    csr get <id>                            # prints CSR PEM to stdout
    csr upload-cert <id> --cert <pem-file>
    csr cancel <id>
    delete <id>
  policies
    list
    set <hostname> --strategy acme-dns --provider <name>
    set <hostname> --strategy manual --cert-id <id>
    clear <hostname>
```

## Web UI

New `Certificates.tsx` route with three sections (tabs or stacked):

1. **Hostnames** — derived from currently-installed apps. Per row:
   hostname, owning app, strategy chip, issuer, expiry (with red badge
   when within 14 days or expired), "Configure" button opening a dialog
   that swaps strategy. This is the page operators land on most often.
2. **Certificates** — table of stored certs (manual + CSR-derived),
   with an "Upload" button (PEM cert + PEM key paste/upload) and a
   "Generate CSR" button (hostname + key type → CSR shown in a dialog
   with copy/download).
3. **DNS providers** — table; add/edit dialog with provider-specific
   fields (Route 53: access key, secret key, optional region).

`AppDetail.tsx` ingress rows gain a small TLS chip showing strategy +
expiry summary, linking to the Certificates page filtered to that
hostname.

## Crates / dependencies

New deps in `crates/core/Cargo.toml`:

- `rcgen` — keypair + CSR generation.
- `x509-parser` — parsing uploaded certs for issuer/notBefore/notAfter.
- `pem` — PEM block encode/decode.

(Or alternatives if there's a clear preference — see open questions.)

## Phasing

1. **Foundation**: spec changes, migration v42, Containerfile + builder
   for Route 53, no behaviour change yet.
2. **DNS provider + ACME-DNS strategy**: storage, OI, CLI, Caddy config
   integration, web UI for providers and policy assignment.
3. **Manual cert upload**: cert validation, cert storage, Caddy
   `load_pem` plumbing, OI/CLI/UI for upload + delete.
4. **CSR flow**: keypair/CSR generation, OI/CLI/UI to begin/get/upload,
   transition to manual.
5. **Visibility**: hostname list joining policies + observed cert
   metadata; AppDetail integration; expiry fault.

Each phase lands behind its own commit set; spec → migration →
implementation → tests within a phase.

## Open questions for the user before implementation

- **DNS provider list**: start with Route 53 only as requested; OK to
  design the storage to make future providers (Cloudflare, etc.) a
  config-only addition? Yes assumed.
- **Crypto crate choices**: `rcgen` for keypair+CSR and `x509-parser`
  for parsing are the obvious picks but pull in a fair bit of code;
  acceptable, or do you want to lean on `openssl`/`rustcrypto` more
  directly?
- **CSR key types**: ECDSA P-256, RSA 2048, RSA 4096 default to
  ECDSA P-256?
- **Cert chain handling on manual upload**: accept full chain,
  validate that leaf matches key and chain order is leaf→intermediate;
  reject self-signed unless `--allow-self-signed` flag? Or accept
  anything and let Caddy serve it?
- **Should the BSL `tls()` ingress builder gain a hint** (e.g.
  `.tls_strategy("dns:route53")`) so app authors can express a
  preference, or is this strictly operator-side? Defaulting to
  strictly operator-side per the architectural decision above.
- **Policy override during operations**: changing a hostname's strategy
  triggers a Caddy config rebuild on the next tick. Acceptable, or do
  we want an explicit "apply" step?
- **Wildcard certs**: a Route 53 + DNS-01 setup naturally enables
  `*.example.com`. Should the policy table support wildcard hostnames
  matching multiple ingresses, or one row per concrete hostname? Start
  with concrete only; add wildcard later if needed.
