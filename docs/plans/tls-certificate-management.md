# TLS certificate management — operator surface

## Goal

Give operators control over how TLS certificates are obtained for ingress
hostnames, and visibility into their state. Four issuance modes:

1. **Caddy ACME with HTTP-01 challenge** — the default for any hostname
   without an explicit policy. Unchanged from today.
2. **Caddy ACME with DNS-01 challenge** — for hostnames that can't or
   shouldn't be reachable on :80 (private ingresses, multi-IP setups).
   Route 53 first; provider list is designed to be extensible.
3. **Manual cert + key upload** — operator brings a PEM cert chain and
   private key obtained out of band. Wildcard certs are permitted in
   this mode (an app *requesting* a wildcard via BSL is not yet
   supported).
4. **CSR flow** — server generates a keypair, hands the operator a CSR
   to take to whatever CA they use, and accepts the signed cert back
   later. The private key never leaves the server. On upload the
   hostname's policy transitions to manual.

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

### Caddy owns HTTP-01 only; the daemon owns everything else

For the **default ACME-HTTP-01** strategy, Caddy continues to acquire,
persist, and renew certs in its own data volume. Unchanged from today.

For **every other strategy** (ACME-DNS, manual, CSR-derived), the
daemon owns the cert. It either acquires the cert itself (running an
ACME client + DNS provider client) or stores the operator-uploaded
cert. In all cases the cert+key live in `tls_certificates` (encrypted
key) and are served to Caddy via Caddy's `tls.certificates.get_certificate`
HTTP module, which calls back into a daemon-hosted endpoint per TLS
handshake (cached by Caddy, so per-handshake hits don't actually
happen for repeat traffic).

The cached Caddy JSON contains only the URL of that endpoint plus the
list of subjects it should answer for — never any cert or key bytes,
never any DNS-provider credentials. The blue/green replay cache stays
fully functional in every strategy.

### Why daemon-owned ACME

Owning the ACME flow ourselves (rather than configuring a Caddy DNS
plugin) means:

- AWS / DNS-provider credentials never leave the daemon. No env-var
  hand-off to Caddy, no creds in the proxy config or its cache.
- Adding new DNS providers is a Rust dep + trait impl, not an xcaddy
  rebuild. (Caddy is rebuilt rarely and the rebuild is slow.)
- One renewal scheduler we control, surfaced via the existing
  observation/fault layer.
- Future possibilities go beyond what Caddy's automation supports
  (custom CAs, internal PKI integrations, etc.).

Cost: we own an ACME client integration (`instant-acme`) and a Route 53
client (`aws-sdk-route53`). Renewal correctness is on us — needs
careful testing.

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

## Database schema (migrations `v42.sql` shipped, `v43.sql` to add)

`v42.sql` is already in place. `v43.sql` (added in phase 2) introduces:

- `tls_acme_accounts(id, directory_url, contact_email,
  account_key_ciphertext, account_url, created_at, updated_at)` —
  persists ACME account state so renewal works across daemon restarts
  without re-bootstrapping the account.
- `tls_certificates.origin TEXT NOT NULL DEFAULT 'manual'` — `'manual'`
  | `'csr'` | `'acme_dns'`. Drives whether the renewal task picks the
  cert up.
- `tls_certificates.acme_account_id INTEGER REFERENCES tls_acme_accounts(id)` —
  for `acme_dns` rows only, identifies the account that issued.

Original v42 schema (already shipped to this branch):

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

Unchanged from today: `Containerfile.caddy` keeps only the `caddy-l4`
plugin. We do not need `caddy-dns/route53` because the daemon performs
DNS-01 itself and serves the resulting cert via `get_certificate`.

## Daemon-owned issuance and serving

A new module `runtime/tls/` holds:

- **`store.rs`** — DB CRUD over `tls_dns_providers`, `tls_certificates`,
  `tls_policies`, plus a new `tls_acme_accounts` table (one row per
  `(directory_url, contact_email)` with the encrypted account key and
  the persisted account URL).
- **`acme.rs`** — drives `instant-acme` for ACME-DNS issuance: account
  bootstrap, order creation, dispatching DNS-01 challenges to a
  provider trait impl, polling, finalize, parse cert metadata,
  insert into `tls_certificates` with `state='active'`.
- **`dns/route53.rs`** — implements the `DnsProvider` trait
  (`set_txt`, `clear_txt`) using `aws-sdk-route53`. Reads creds from a
  `tls_dns_providers` row.
- **`renewal.rs`** — background task that wakes hourly, queries certs
  with `not_after - now() < threshold` (default: 1/3 of total
  lifetime), and re-runs the issuance flow. Sleep schedule capped so
  we don't burn through ACME rate limits.
- **`serve.rs`** — implements the `tls.certificates.get_certificate.http`
  endpoint contract: receives an SNI hostname, returns PEM cert+key.

### get_certificate endpoint transport

The daemon exposes the endpoint on a Unix socket bind-mounted into
Caddy's container, alongside the existing admin socket. Path:
`{data_dir}/caddy-cert/{slot}/cert.sock`. Caddy's
`tls.certificates.get_certificate.http` module accepts URLs whose
host portion is a Unix socket via standard Caddy URL conventions
(verify before commit; if not, fall back to a local TCP listener on
the proxy bridge gateway).

The endpoint is read-only (lookup by SNI hostname) and authenticated
by socket file permissions: only Caddy's container can connect.

## Caddy config changes

`crates/core/src/system/caddy/config.rs` `build_caddy_config` gains a
single optional addition: when at least one hostname has a non-default
strategy, emit a `tls.certificates.get_certificate` array with one
entry of module `http`, URL pointing at the daemon socket, and the
list of those hostnames as `subjects`. No automation-policy fan-out,
no `load_pem`, no DNS-provider issuer config — Caddy is just a fetcher.

The cached Caddy JSON contains only the URL + subject list, no
secrets. The cache continues to function in every strategy.

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

- `instant-acme` — async ACME (RFC 8555) client.
- `aws-sdk-route53` — Route 53 API client. Heavy, but the canonical
  choice; can swap for a hand-rolled SigV4 + REST shim later if size
  is a concern.
- `rcgen` (0.14.x) — keypair + CSR generation (for both phase 4 and
  ACME orders).
- `x509-parser` — parsing certs for issuer/notBefore/notAfter and SAN
  inspection.
- `pem` — PEM block encode/decode.

## Key types

Only ECDSA P-256 is offered initially. The `key_type` enum is shaped
so adding ML-DSA (FIPS 204) later is a single-variant + match-arm
change:

```rust
pub enum KeyType {
    EcdsaP256,
    // Future: MlDsa65, MlDsa87 — gated on rcgen's
    // `aws_lc_rs_unstable` feature, public-CA acceptance, and Caddy /
    // rustls cert-loading support. Not viable today (Caddy 2.11 +
    // rustls do not currently serve ML-DSA leaf certs and no public
    // CA issues them).
}
```

RSA is intentionally not offered — there's no need for it given
ECDSA-P256 is universally supported by ACME, browsers, and Caddy, and
RSA-4096 keys are large and slow.

## Cert validation rules

On manual-cert upload (`/tls/certificates/upload-manual`) and on
CSR-cert upload (`/tls/certificates/csr/upload-cert`):

- **Key match** (manual only): the leaf cert's subject public key must
  equal the public key of the supplied private key. CSR uploads check
  against the stored key.
- **SAN coverage**: the leaf cert's SubjectAlternativeName (DNS) entries
  must contain the target hostname, *or* a wildcard SAN that covers it
  (`*.example.com` covers `foo.example.com` but not `example.com` or
  `a.b.example.com`, per RFC 6125). Reject the upload otherwise.
- **Self-signed**: detected when issuer DN equals subject DN and chain
  length is 1. Allowed, but the response includes a `warnings` array
  containing `"self_signed"` and the row is marked accordingly so the
  UI can flag it.
- **Validity dates**: parsed and stored. An already-expired cert is
  rejected (`not_after < now()`); a not-yet-valid cert is accepted but
  a `warnings` entry is added.

For wildcard manual certs (`*.example.com`), the operator binds the
cert to a specific concrete hostname like `foo.example.com`; the policy
table still keys on the concrete hostname declared by an app's
ingress. SAN coverage validation accepts the wildcard.

## Phasing

1. **Foundation**: spec changes, migration v42 for tls tables, no
   behaviour change yet. Done.
2. **DNS provider + daemon-owned ACME-DNS**: storage CRUD, ACME client
   integration, Route 53 provider impl, renewal background task,
   `get_certificate` endpoint, Caddy config integration, OI, CLI, web
   UI for providers and policy assignment. Largest phase by far.
3. **Manual cert upload**: cert validation (key match, SAN coverage,
   self-signed warning), encrypted key storage. Reuses phase 2's
   `get_certificate` endpoint and the `tls_certificates` table.
   Adds OI/CLI/UI for upload + delete.
4. **CSR flow**: keypair/CSR generation via rcgen, OI/CLI/UI to
   begin/get/upload-cert/cancel. Reuses phase 2's serving path.
5. **Visibility**: hostname list joining policies + observed cert
   metadata; AppDetail integration; expiry fault.

Each phase lands behind its own commit set; spec → migration →
implementation → tests within a phase.

## Future enhancements (out of scope for this plan)

- **Short-lived certs**: opt-in 6-day Let's Encrypt cert profile per
  hostname or globally per ACME account. The renewal task already
  copes with arbitrary lifetimes (it renews at remaining-third), so
  the cost is roughly: a `profile` field on `tls_acme_accounts` or per
  policy, plus passing `profile` through `instant-acme`'s order params.
- **IP-address certs**: Let's Encrypt now issues certs for public IPv4
  and IPv6 addresses. Adds a parallel "policy by IP" path next to
  "policy by hostname" — the data model would extend to use a
  `subject` enum (DNS name / IPv4 / IPv6) instead of a bare `hostname`
  column. SAN-coverage validation already needs to learn iPAddress
  SAN entries. Worth a small follow-on plan once the DNS-name path
  is solid.
- **Additional DNS providers**: Cloudflare, RFC 2136, etc. — each is
  a Rust crate + a `DnsProvider` trait impl, no Caddy rebuild.
- **Internal CA / private PKI**: a strategy where the daemon issues
  its own certs from a configured root.
- **Wildcard ACME issuance**: requires DNS-01 (already supported), but
  needs BSL-side support for wildcard ingresses first.
- **HTTP-01 ownership migration**: optional later move of HTTP-01 into
  the daemon too, freeing Caddy to be a pure proxy. Requires
  `/.well-known/acme-challenge/*` reverse-proxying.

## Decisions confirmed

- Four modes, with default-HTTP-01 explicitly retained as one of them.
- `rcgen` for keypair+CSR, `x509-parser` for parsing.
- ECDSA P-256 only at first; data model leaves a hole for ML-DSA later.
  PQC is a "no" today: Caddy 2.11 + rustls don't serve ML-DSA leaves,
  no public CA issues them, and rcgen's ML-DSA support is behind an
  unstable feature flag. Revisit when at least Caddy and one public CA
  are ready.
- No BSL surface — strategy is strictly operator-side.
- Manual wildcard certs allowed; no wildcard *issuance* (ACME) yet; no
  BSL wildcard-ingress support yet.
- SAN-coverage check on every cert upload; self-signed allowed with a
  warning.
- Policy changes apply on the next reconciler tick; no explicit "apply"
  step.
