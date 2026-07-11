# Seedling Windows Runtime Threat Model

This document describes what the seedling Windows runtime tries to defend
against, what it does not, and the mechanisms it uses. It is descriptive, not
normative: the authoritative requirements live in
`docs/spec/runtime-windows.md`, and rules are referenced by their `win[...]`
identifiers.

It is a companion to the base threat model (`docs/threat-model.md`). The
operator interface, BSL sandbox, secret-parameter, TLS, and audit surfaces
carry over unchanged, and their treatment is not repeated here. This document
covers what changes when workloads are native Windows processes instead of
Linux containers.

## Audience

- Operators evaluating whether the Windows runtime fits their deployment
  model, and the security reviewers of those deployments.
- Reviewers assessing whether a change preserves the boundaries described
  here.
- Future contributors deciding which class of threat a new feature belongs
  to.

## Trust model

Principals on a Windows Server host:

1. **The host.** The Windows kernel, the SCM, BFE/WFP, NTFS, and whatever
   GPO baseline and EDR the deployment runs. Trusted completely; a
   compromised host ends every protection below.
2. **The seedling daemon (seedlingd).** The only auto-start seedling
   service, running under its own virtual service account rather than
   LocalSystem (`win[identity.scm-entry]`). Pure control plane: it computes
   ACLs and WFP filters, creates and commands supervisors, and sits in no
   data path.
3. **Supervisors (seedpod).** One per instance, each under the instance's
   virtual account. A supervisor owns its instance's Job Object, listeners,
   and relay. It is trusted infrastructure, but only within its instance's
   scope: its compromise is that instance's compromise, not the fleet's.
4. **Operators.** Authority over seedling is total, exactly as in the base
   model: operator authorisation remains the trust boundary, and audit is
   after-the-fact.

Untrusted, as in the base model: app definition authors (the BSL sandbox is
unchanged) and workloads.

The structural difference from Linux: workloads are **native processes**,
not containers. There is no kernel namespace isolation, no private rootfs,
no image-scoped view of the filesystem. Containment is built from identity —
virtual-account SIDs, stripped tokens, NTFS ACLs, and WFP filters — and is
therefore **discretionary**: it binds non-privileged code and is void
against administrators (`win[wfp.honesty]`).

## What we defend against

### W1. Cross-workload network reach outside the mount graph

Connections into the seedling prefix are denied by default and allowed only
where the mount graph says so, keyed on (SID, address). Filters are
persistent WFP objects under the seedling provider GUID: enforcement lives
in BFE and holds with the daemon stopped, crashed, or mid-upgrade
(`win[wfp.provider]`, `win[wfp.default-deny]`, `win[wfp.allows]`).

### W2. Seedling-address squatting

A process that has not been granted an address cannot bind one inside the
seedling prefix, so a workload's private listener cannot be pre-claimed and
a supervisor cannot be made to relay traffic to an imposter. The supervisor
additionally verifies at readiness that every `BIND_ADDRESS` entry is held
by a process inside the instance's Job Object (`win[wfp.default-deny]`,
`win[net.bind-verify]`).

### W3. Cross-workload file access

Seedling-managed data directories, volume roots, and secret files carry
ACLs naming the owning instance SID, the daemon's service SID, SYSTEM, and
Administrators — no other principal — with inheritance from parent
directories broken on creation (`win[identity.file-permissions]`).

### W4. Workload privilege creep

Workloads never run elevated: the supervisor spawns them under a stripped
token narrower than its own. Workloads cannot mutate WFP state (BFE
mutation requires administrative rights), other instances' processes or
volumes, or ingress configuration (`win[identity.non-admin]`,
`win[identity.file-permissions]`). Commands executed in an instance by
actions get the same stripped token, not a wider one (`win[action.exec]`).

### W5. Supervisor impersonation over the control pipe

The pipe namespace is first-come-first-served, so neither end trusts it
blindly: the supervisor ACLs pipe-instance creation to its own SID (plus
SYSTEM and Administrators), and the daemon verifies on every connect that
the pipe server matches the recorded supervisor's PID and process start
time before trusting anything it reads (`win[supervisor.pipe-trust]`).

### W6. Malformed or tampered images reaching the kernel

Attaching a VHDX hands the blob to the kernel filesystem parser. The
uncompressed digest is verified after decompression into the store and
re-verified before every attach; a mismatch quarantines the entry, files a
fault, and never attaches. Attach is unconditionally read-only
(`win[artifact.verify]`, `win[artifact.attach]`).

### W7. Identity residue

Service registrations, volume ACEs, and WFP filters that match no live
instance record or in-progress operation are swept by the GC pass, so stale
grants do not accumulate into an ambient attack surface
(`win[identity.gc]`).

## What we do not defend against

### WN1. Administrators

Everything above is discretionary enforcement against non-privileged
processes, not a sandbox. An administrative process can delete the WFP
filters, rewrite the ACLs, and kill or impersonate any seedling component.
This is stated in `win[wfp.honesty]` and is the Windows restatement of the
base model's "operator authorisation is the trust boundary".

### WN2. Per-connection authentication on mounts

A mount compiles to a WFP allow; the relayed byte stream itself carries no
authentication. Client identity for HTTP traffic is layer-7
(X-Forwarded-For from ingress). This is parity with the Linux DNAT model,
not a regression.

### WN3. The host's external network posture

The host remains default-open to the external network; outbound-deny policy
is out of scope for v1 (`win[platform.non-goals]`).

### WN4. Workload–supervisor mutual protection

In v1 a workload and its supervisor share a SID and are mutually
unprotected: a compromised workload can kill its supervisor or interfere
with the supervisor's listeners. The blast radius is the instance itself —
supervisor death presents as the pod being down (`win[supervisor.crash]`) —
not other instances. Separating the supervisor under its own SID is
recorded future hardening (`win[identity.non-admin]`).

### WN5. The ambient read surface of native processes

Without namespaces or a private rootfs, a workload can read whatever the
host leaves world-readable: OS binaries, machine-wide configuration,
process lists. ACLs defend seedling-managed state and other instances'
data; they do not shrink the host's ambient surface the way a container
rootfs does. AppContainer sandboxing is recorded as future hardening, not
present protection.

### WN6. Defender real-time exclusions

The image store and volume roots carry a documented real-time-scanning
exclusion (on-demand scanning retained) so database and image I/O is not
throttled (`win[artifact.store]`). A file implanted under an excluded path
is found only by on-demand scans. This is a deliberate, documented trade;
deployments whose EDR policy cannot accept it should not exclude, and
accept the I/O cost.

### WN7. Kernel, SCM, or BFE bugs; physical access

As in the base model: seedling trusts the host, and disk encryption is the
operator's responsibility. A workload exploiting a kernel or platform CVE
to elevate is out of seedling's reach; patch the host.

## Governance ledger

Facts a deployment's security review should know in advance, so none of
them is discovered in a SIEM first:

- Service installation fires audit event 4697 per registration — including
  once per dynamic-Job invocation (`win[identity.dynamic-jobs]`) — and
  service installation is a persistence technique EDR products watch.
  Baseline the `seedling-` service-name prefix.
- Service deletion is not distinctly audited: SIEMs observe creations
  without matching removals. No invisibility is claimed anywhere in the
  design.
- There is no credential surface: virtual accounts have no passwords, no
  account objects, and appear in no account-review reports
  (`win[identity.virtual-account]`).
- The daemon–supervisor pipe is authenticated by ACL plus server-identity
  verification (`win[supervisor.pipe-trust]`); an elevated process can
  still impersonate, consistent with WN1.

## Reviewing changes against this model

1. Does a new WFP filter, ACE, or service registration correspond to
   something in the mount graph or an in-progress operation — and will the
   GC sweep find it if orphaned? (W1, W7)
2. Does anything hand a workload, an exec'd command, or a supervisor more
   privilege than the stripped-token / virtual-account slice? (W4)
3. Does a new artifact or attach path skip digest re-verification before
   handing bytes to the kernel? (W6)
4. Does a change put a seedling process into a data path it cannot be
   restarted out of? The daemon and supervisors stopping without dropping
   traffic is a designed property (`win[special.upgrade]`), not an
   accident.
5. Does a new feature grow the supervisor? Its reliability budget is the
   workload's (`win[supervisor.crash]`); features that can live in the
   daemon should.
6. Does anything trust a pipe name, service name, or address to be
   unforgeable without verifying the identity behind it? (W2, W5)
