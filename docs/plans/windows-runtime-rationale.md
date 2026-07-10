# Windows Runtime: Design Rationale

Non-normative companion to `runtime-windows.md`. Records alternatives considered and rejected, fallback positions, and material for the Windows threat-model document. The spec documents what the runtime does; this documents why, and what it deliberately does not do.

## Rejected approaches

**Rejected: WSL2.** Mirrored networking has removed the UDP/proxy objections, but unattended start remains scheduled-task duct tape, the root filesystem remains ext4-in-VHDX, and the failure modes of a VM layer would be ours to own at 25 field hosts. Re-evaluate only if workload requirements change.

**Rejected: Windows containers.** No trustworthy upstream images for the stack; gigabyte base layers; we would maintain a bespoke image toolchain to containerize software we already run natively.

**Rejected: plain local accounts** (audit findings: 4720 events, account-review noise, password-age GPO interaction) and **AppContainer SIDs** (a real default-deny sandbox; running Postgres/Node inside one is a research project). AppContainer is recorded as future hardening.

**Rejected: wintun + userspace netstack.** Its distinguishing capabilities (ICMP, undeclared ports, cross-host reachability) are unneeded or antigoals; costs are a signed driver and AV/VPN interaction risk in the field. The addressing scheme is prefix-shaped, so a driver data plane could later replace the socket layer without respeccing addresses.

## Fallback positions

**Escape hatch: direct-dial mounts.** With workload listeners inside the prefix, a mount can be compiled to a WFP allow (A's SID → B's private address) bypassing the supervisor relay, trading backlog-shaping and address stability for zero relay hops. Candidate: app→Postgres. Not v1; requires no address respec.

**Retreat positions for dynamic-Job identity.** If per-invocation service churn proves noisy (SIEM escalations, marked-for-delete accumulation, dispatch latency): first retreat is stable service names per `(app, job-definition)` — lifecycle-operation concurrency means same-Job instances coexist only via concurrent shell attaches, which can suffix — reducing registration to once per Job definition. Second retreat is dynamic Jobs sharing the parent app's SID (no registration; Jobs become indistinguishable from the app in the ACL/WFP model). The backup rework removes the main scheduled churn source, so the trigger may never fire.

**No blue/green for special services (v1).** A zero-downtime ingress upgrade would need a Seedling-held front bind relaying to Caddy, putting a Seedling process in the public data path — the daemon could then never restart without traffic risk. Letting Caddy (and CoreDNS) hold their own binds inverts the trade: binary upgrades cost a brief outage, and in exchange every Seedling process can stop without dropping traffic. Workloads are unaffected: their supervisor already holds the service-address binds, so replacement generations switch behind the relay at no extra cost (`win[deploy.replace]`).

**seedpod as data plane.** Recorded trade: the supervisor's reliability budget equals the workload's, and it relays all mount traffic. Mitigations if it pinches: smaller job-holder process; direct-dial mounts; both invisible to spec surfaces.

## Governance ledger

Input for the Windows threat-model document: service installation fires event 4697 per registration — including per dynamic-Job invocation under the chosen design — and is a persistence technique EDR watches; service deletion is not distinctly audited, so SIEMs observe creations without matching removals. No invisibility is claimed. There is no credential surface: nothing has a password, nothing appears in account-review reports.
