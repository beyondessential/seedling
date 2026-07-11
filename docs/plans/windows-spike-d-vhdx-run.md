# Spike D: Run Tamanu from a Read-Only VHDX

Budget: half a day; mostly already exercised by the Tamanu `vhdx-pack`
build work. Environment: any Windows Server 2019+ box with a produced
Tamanu artifact.

## At stake

- `win[artifact.attach]` — read-only attach as the activation mechanism,
  including NTFS's mount-time write urges being fully neutralised.
- `win[artifact.verify]` — whether re-verifying the uncompressed digest
  before *every* attach is genuinely negligible on field-class disks.
- `win[artifact.rebase]` — `WorkingDir`/`PATH` rebasing onto the mount
  point beneath `vhdx.rootDir`.
- `win[action.env-hygiene]` — the minimal-base-plus-artifact-env
  construction actually suffices to run the workload.

## Experiments

1. **Store and verify.** Pull the artifact, decompress into a digest-named
   store entry, verify the uncompressed SHA-256. Time the pre-attach
   re-verification on a realistic image size and on disk comparable to the
   field hosts; record cold-cache and warm-cache numbers.
2. **Attach.** `AttachVirtualDisk` read-only with a folder mount point and
   no drive letter. Confirm the dirty bit and `$LogFile` state of the image
   are untouched after attach/detach cycles (hash the file before and
   after), and that a second concurrent read-only attach of the same image
   behaves.
3. **Launch.** Resolve entrypoint from the config blob, rebase
   `WorkingDir` and `PATH`, construct the hygiene environment, spawn under
   a Job. Confirm Tamanu starts with nothing inherited from the host
   toolchain.
4. **Writable-assumption hunt.** Run Tamanu through a representative
   workload with Process Monitor filtering for write attempts under the
   mount point. Every hit is an upstream fix (TMP/TEMP redirection, log
   directories onto volumes, cache paths); enumerate them here and file
   them against the Tamanu build.
5. **Failure modes.** Detach while the workload is running (simulates GC
   racing an attach) and confirm the failure is clean and attributable;
   corrupt a store entry and confirm the pre-attach verify quarantines it
   and never attaches (`win[artifact.verify]`).

## Exit criteria

- Tamanu runs correctly from a read-only attach with zero writes under the
  mount point (or a complete, upstream-filed list of the writes found).
- Per-attach verification cost recorded with a verdict. If it is not
  negligible, propose the amendment to `win[artifact.verify]` (for
  example, verify-on-first-attach-per-boot) rather than silently skipping.
- Attach/detach leaves the image bit-identical.

## If it fails

- Irreducible writable-app-dir assumptions in Tamanu: fix upstream; the
  runtime does not grow a writable overlay. A copy-on-attach escape hatch
  would forfeit the immutability property and is not on the table for v1.
- Verification cost too high: cadence is the lever (per-boot or per-store
  mutation instead of per-attach), never skipping verification entirely —
  the digest check is what stands between a tampered store entry and the
  kernel filesystem parser.
