# Spike C: Networking on a Worst-Case Image

Budget: 1–2 days. Environment: a disk image of the least-friendly of the 5
deployments — real GPO baseline, real EDR, real DNS configuration. Not a
lab box; the point is coexistence, not correctness in a vacuum.

## At stake

- `win[net.prefix]` — carries `[spike: v4 fallback stance]`; this spike
  answers Q1 (per-service 127/8 aliases versus no v4 fallback).
- `win[net.resolver]` — NRPT rule application, scoping, and removal on a
  host that may already carry corporate NRPT policy.
- `win[wfp.provider]`, `win[wfp.default-deny]`, `win[wfp.allows]` —
  persistent provider/sublayer install, connect-layer and bind-layer
  default-deny, and above all coexistence with the deployment's actual EDR
  in the ALE layers.
- `win[net.bind-address]`, `win[net.bind-verify]`, `win[net.listener]`,
  `win[net.mount]` — the addressing scheme end-to-end with a real Tamanu
  build.

## Experiments

1. **Prefix and aliases.** Derive the ULA prefix from `MachineGuid`; add
   service and private addresses as loopback aliases with `skipassource`.
   Confirm outbound flows from ordinary host software never select a
   seedling address as source, and that the aliases survive interface
   resets. Note alias count at realistic fleet scale (every service +
   instance) and any slowdown in address enumeration.
2. **v4 fallback (Q1).** Establish Windows 127/8 behaviour: whether binding
   an arbitrary 127.x.y.z requires an explicit alias, and whether a
   v6-incapable dialler on the host can reach a per-service 127.x alias
   relayed by the supervisor. Decide: per-service v4 alias, or no v4
   fallback. Write the answer into `win[net.prefix]` and drop the spike
   tag.
3. **NRPT.** Install the seedling-zone rule; confirm only that zone routes
   to the seedling resolver, global resolution is untouched, existing
   corporate NRPT rules still apply, and removal restores the prior state
   byte-for-byte. Test idempotent reinstallation.
4. **WFP under EDR.** Install the persistent provider, sublayer, and
   filters. Confirm: loopback traffic classifies at the ALE connect and
   bind layers on this image; the default-deny holds with seedlingd
   stopped; the EDR's own callouts and filters continue functioning (and
   the EDR does not flag or strip ours); sublayer weight interactions are
   understood; a provider-scoped sweep removes everything cleanly.
5. **BIND_ADDRESS end-to-end.** Run a real Tamanu build under the harness:
   inject `BIND_ADDRESS`, verify the listeners land inside the Job via the
   extended TCP table, verify the supervisor relay path (dial service
   address, reach workload), verify a non-granted process can neither bind
   a prefix address nor dial one, and confirm `win[net.bind-verify]`'s
   fault fires when the workload is made to bind bare loopback instead.
6. **Event-log profile.** Capture what the EDR and Windows firewall logs
   record for all of the above; feed anything noisy into the threat-model
   document's governance ledger.

## Exit criteria

- Every mechanism works on the worst-case image, with the EDR active, and
  survives daemon stop/start: remove the spike tag from `win[net.prefix]`.
- Q1 answered in the spec, plan updated.
- A coexistence note per EDR/GPO surprise, recorded here and in the doctor
  checklist (each becomes a `seedling doctor` preflight probe).

## If it fails

- ALE loopback classification defeated by the EDR on this image: that host
  profile is unsupportable as specced — record it, check the remaining 4
  deployments, and only then consider design changes.
- `skipassource` not honoured or aliases unstable: the wintun/netstack
  path stays rejected; the fallback is fewer, stabler aliases (service
  addresses only, private listeners on distinct ports of one address),
  which respecs `win[net.bind-address]` but not the mount model.
- NRPT conflicts with corporate policy: scope the seedling zone narrower
  or document the GPO precedence requirement as a deployment prerequisite.
