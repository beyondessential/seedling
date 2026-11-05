---
status: draft
---

# Integrate Canopy

Broad strokes of how Seedling adopts Canopy for TLS certificates, backups, DNS records, and a proxied OIDC/IdP facility, before splitting into focused implementation cards.

## Starting point

What each side has today, as read from Seedling main, Canopy main, the Canopy A5 branch, and bestool main.

- **Seedling has no Canopy identity.** A connected client (bestool) offers to carry requests under its own identity; the relay is a generic outbound HTTP conduit (16 MiB body cap, 60 s deadline, 8 in flight). Seedling reports every sixty seconds as its own source with four fixed checks. The report response's backup list is specified as always empty for Seedling.
- **Canopy identities belong to the box.** One identity per machine, no sub-identities. Which application a request concerns comes from the request body (name, `application_type`), never the credential. Applications are addressed by type, and Canopy assumes at most one application of a type per machine.
- **Certificates (Canopy CRT).** `POST /certificates/request` with a CSR for exactly one name, no wildcards. The device holds the key. Idempotent per (name, key): 202 pending, 200 with chain. Canopy renews (ARI or one third of life left) and the device collects; `key_must_be_replaced` forces a new key. Authorised per application by the `may_manage_tls` grant and the application's group's domains. Entitlements come back on every status response.
- **DNS names (Canopy ADR).** `POST /names/register` with a name and addresses; A/AAAA replaced wholesale, empty list withdraws. Same authorisation chain with `may_manage_dns`. Canopy does not verify the addresses. bestool only registers when an operator runs a command.
- **Backups (Canopy BAK).** Per-type: the device declares capabilities, fetches the target (S3 bucket, prefix, kopia repo password) and short-lived STS credentials scoped to the prefix, and reports progress and outcome. Canopy owns schedules, retention and maintenance; the device never deletes. `backup_now` on the status response is hard-coded to the `alertd` source. bestool runs kopia through a loopback re-signing proxy that refreshes credentials.
- **Identity tokens (Canopy A5, not merged).** `POST /identity-tokens` with an audience and optional `application_type`, returning a short-lived ES256 JWT. Grants per relying party, per machine or per application. bestool is to expose a loopback token endpoint; not built yet.
- **Seedling TLS.** Per-hostname strategies (HTTP-01 default, ACME DNS-01 via a provider, manual, CSR). The runtime already generates keys and CSRs, stores keys encrypted, serves certificates to Caddy by SNI, and has one decision function behind issuance, renewal and the hostname view.
- **Seedling backups.** A backup-app framework (providers, strategies, scheduling, `backup_failed`), slated for removal by the Seedling in production PRD. Operation-scoped volume bindings stay. `/var/lib/seedling` is backed up by something other than Seedling.

## Behaviour

### Cross-cutting

- Every Canopy call Seedling makes goes through the relay, under the offering client's identity. No live offer, no Canopy features; each strand says what it does in that case.
- An app definition declares its Canopy application type in BSL. That type is what Seedling sends as `application_type` for the app's certificates, DNS names and identity tokens, and what it reports the app under.
- Seedling reports each app with a declared type to Canopy as an application, so Canopy knows it and can hold grants for it.
- Canopy addresses applications by type, one per machine. The first app to claim a type holds it. A later app declaring the same type gets a non-critical fault and no Canopy features, and the first carries on unaffected. When the holder is removed, the next claimant takes the type over, so an app can be rotated by installing its replacement before removing it.

### Certificates

- Operator policies (ACME DNS-01, manual) always win over Canopy for the hostnames they cover.
- For a hostname with no operator policy, Canopy is tried first whenever an offer is live and the entitlements say the hostname's app may have a certificate for it. HTTP-01 stays the fallback: while Canopy has no certificate for the name, the proxy's own HTTP-01 issuance applies.
- An operator can bind a hostname or pattern to a Canopy-only policy, which drops the HTTP-01 fallback. This covers a hostname Seedling does not yet receive traffic for, where HTTP-01 cannot succeed.

### Backups

- Seedling performs backups of its apps' data itself, with kopia, against the target and credentials Canopy hands out.
- An app declares its backups in BSL: each names a Canopy backup type, the volumes it covers, and optionally closures that quiesce and resume the app around the snapshot.
- Seedling declares those types to Canopy as its own capabilities. Canopy records capabilities per source and sends each type's `backup_now` only to the source that declared it, so bestool keeps backing up `/var/lib/seedling` and host-level data alongside.

- Restores are operator-run on the host (ctl, OI, web) for a backup type and snapshot, within the restore window an operator opens in Canopy. Seedling stops the app, swaps the data in, and keeps the previous data aside.

### DNS names

- The operator sets the host's public address or addresses once. Registration is off until the operator turns it on.
- With registration on, every TLS ingress hostname that Canopy allows the app to publish (`may_manage_dns`) is registered with those addresses.
- A "detect public IP" helper suggests an address for the operator to confirm. It never sets one on its own.

### Identity tokens

- Seedling serves a token endpoint reachable from app containers. It identifies the calling app by container identity and mints a fresh token per call through the relay, as that app's Canopy application.
- Seedling passes any audience through. Canopy's per-application grant is the only gate.

## Trade-offs

- **Relay rather than Seedling's own credentials.** Canopy allows one identity per machine, and bestool holds it. Borrowing bestool's device key would couple Seedling to bestool's on-disk format and put Canopy credentials where Seedling's state lives. Per-agent sub-identities would be a large Canopy change. The cost is that everything here depends on bestool being connected.
- **Type declared in BSL.** Definitions are moving to the app repos, so the app's own repo states what it is to Canopy. Using the app name would tie Seedling naming to Canopy's type slugs. Operator configuration would repeat the same fact on every host.
- **Seedling runs kopia.** Seedling owns the app data and how to make it consistent, and the operation-scoped volume bindings give a backup run a path without inventing a name. Handing snapshots to bestool would split one backup across two daemons. An in-process uploader would diverge from the repository format the rest of the fleet uses.
- **Automatic certificate path.** Canopy entitlements already express which names an app may have, so a per-hostname binding would duplicate that on every host.

- **BSL-declared backups.** An app knows which volumes belong together and what has to happen for them to be consistent. Implicit per-app or per-volume backups would either over-collect or leave consistency to chance.
- **Routing by declared type.** Hard-coding a second source name repeats the existing `alertd` special case. Forwarding through bestool would add a second hop to the one path that should keep working when bestool is unwell.
- **Operator-set addresses.** Seedling cannot reliably learn a public address behind NAT. Per-site-ingress addresses would add configuration that no deployment has asked for yet.
- **Operator policies beat Canopy, and HTTP-01 stays as the fallback.** An operator who has bound a policy has said how that name gets its certificate. Canopy fills the gap in front of the default rather than replacing it, so a host whose Canopy entitlements are missing or misconfigured still serves. The Canopy-only policy is the one way to opt out of the fallback, because nothing else can express "never try HTTP-01".
- **Fault duplicates rather than refuse them.** Refusing the install of a second app of a type would make it impossible to stand a replacement up beside the old app.
- **Audiences gated by Canopy only.** Grants are already per application and per relying party in Canopy, and a second list in the definition would have to be kept in step with them.
- **Operator-run restores.** This matches how bestool restores work today. Restores queued from Canopy would need a new delivery path, and nothing has asked for that yet.
- **Token endpoint on the app network.** A file mounted into the container needs Seedling to know the audiences in advance and to keep it refreshed. A token minted by a BSL call goes stale once it expires.

## Open questions

- [x] How does Seedling reach Canopy for these calls: through the existing relay, or with credentials of its own? Relay.
- [x] How does a Seedling app map to a Canopy application (type and key)? Type declared in BSL.
- [x] Which component performs a backup of a Seedling app's data, and with what? Seedling, with kopia.
- [x] How is the Canopy certificate path chosen for a hostname: an operator-bound strategy, or automatic when entitled? Automatic.
- [x] Which addresses does Seedling register for a hostname, and when? Operator-set per host, when the operator enables it and Canopy allows it.
- [x] How does a workload ask Seedling for an identity token? An endpoint on the app network.
- [x] Which audiences may an app request tokens for? Anything Canopy grants.
- [ ] What changes does Canopy need, and in which order? So far: capabilities and `backup_now` recorded and routed per source.
