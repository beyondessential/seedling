# Windows log store: plan

Companion to `runtime-windows-logs.md`. The spec states the contract; this records how it is realised, why the shape was chosen, and what is unsettled.

The Linux runtime writes container output and its own `rt.*` breadcrumbs into journald and reads them back with field matches, which gives indexed filtering, retention, rotation and follow for free. Windows has no equivalent, so the store is ours to build. The goal is not to reimplement journald — it is to serve the operator interface's log surface no worse than journald does for the queries that surface actually offers.

## What the surface actually asks for

The implemented query surface is narrow: app, app+resource, app+resource+instance, or an infrastructure component, plus `follow` and `tail`. There is no text search, no time range, and no level filter anywhere — not in the operator interface, not in the CLI, not in the web UI.

That is what makes an index unnecessary. Partitioning the store on exactly those fields turns a filter into a path selection, so the cost of a filtered query is proportional to the number of matching producers rather than to the number of stored records. journald needs an inverted index because every unit's records share one journal; partitioning avoids the problem instead of solving it.

## Capture: the binary logging driver

containerd does not persist logs. It hands a task's output to whatever the client asks for, and runtime v2 offers four URI schemes, of which `fifo` is Linux-only and `npipe`, `file`, `binary` and `binary-v2` are available on Windows.

Use `binary-v2`. The shim launches the named binary and hands it the container's output streams; the process is a child of the shim, and the shim is the per-instance supervisor that already outlives both containerd and seedlingd (`wcr[shim.ownership]`). Log capture therefore inherits daemon-independence from the same mechanism the rest of the runtime rests on, which is what `wlog[producer.capture]` requires.

The alternatives fail on that property or on retention:

- **`npipe`** puts seedlingd in the capture path. Output produced while it restarts is lost, and a pipe whose reader has gone can block the writer — the workload stalls behind its own logs.
- **`file`** survives seedlingd, but it is a plain file with no rollover, so nothing bounds it.

The cost is honest and worth stating: this ships a second binary and runs one small seedling process per container. The container design was partly argued on the shim replacing a per-instance seedling supervisor; a per-instance seedling process comes back, much smaller — it appends and rolls, nothing else.

The writer learns only its namespace and container ID from containerd, so the identifying fields and rollover thresholds of `wlog[producer.capture.config]` ride in the `binary://` URI's query string when seedlingd creates the task. The container ID it is handed is what fills the record's unit field.

## Layout and format

Segments live under a tree keyed on the filter dimensions, so a target resolves to a subtree. seedlingd's own records are written into the same tree under their target's path, with app-scoped records (replay markers and other untargeted breadcrumbs) at the app level; a query for one instance merges the instance, resource and app levels — a handful of files, all chosen by path.

Records are newline-delimited JSON in the shape the operator interface already streams, which makes the read path a merge-and-forward rather than a parse-transform-serialise. It also keeps the store greppable on a field host, which matters most when the control plane is the broken thing.

Segments are numbered rather than timestamped. Numbers are stable, sort correctly, allow rollover on either a size or an age trigger without the name disagreeing with the contents after a clock step, and still support time queries: segments are time-ordered within a producer, so a range prunes by binary search over segment numbers, reading the first record of each candidate. If that ever needs to be O(1), the rotator can maintain a per-directory manifest of number to time span, which fits naturally with seedlingd owning rotation.

`tail` must be a reverse read from the end of the newest segment — read the last block, split on newlines, discard the partial leading line. Implementing it as a forward scan would satisfy the tests and violate `wlog[read.tail]` on any real host.

## Rotation, compression, retention

The split is forced by the platform. A file open for writing cannot be renamed or deleted by another process on Windows unless every handle holder opened it with `FILE_SHARE_DELETE`, and even then name reuse is awkward — so the writer must roll its own active segment (`wlog[retain.rollover]`), because nobody else can close that handle. Everything after sealing is seedlingd's: sealed segments have no writer, so compressing, retaining and deleting them raises no sharing question at all.

Two consequences the implementation has to respect:

- Leave the newest sealed segments uncompressed, so `tail` and `follow` never decompress. Deep history pays a decompress, which is the right place to pay it. Frame-based compression would preserve seekability if that ever matters.
- Compress to a temporary name and rename into place, then unlink the original. Otherwise a seedlingd crash mid-compress leaves a segment present in both forms and a reader emits its records twice.

Retention is not applied while seedlingd is down, which is correct — capture must not stop because the control plane did — but unbounded growth on a small field disk could take the workloads out. Hence the writer-side backstop of `wlog[retain.backstop]`: a generous absolute cap on a producer's own segments, well above where ordinary retention reclaims, so the failure mode is bounded rather than fatal.

## Open

| # | Question | Lean |
|---|----------|------|
| L1 | Confirm the `FILE_SHARE_DELETE` behaviour the rotation split rests on: what a second process can and cannot do to a segment the writer holds open. | Spike it before building the rotator; the whole division of labour depends on it. |
| L2 | Rollover thresholds, retention policy, and the backstop cap. | Wants a real workload's output rate to choose sensibly; pick provisional values and revisit after the first pilot. |
| L3 | Compression codec, and whether to frame it for seekability. | zstd, unframed, until deep-history reads are shown to matter. |
| L4 | Whether the writer is its own binary or a mode of an existing one. | A mode flag on an existing binary avoids a second artifact to build, sign and ship. |

## Deferred

**Message text search.** journald does not index message text either — `journalctl -g` is a scan — so this is parity, and path selection has already narrowed the input, meaning a scan here reads less than journald's would. It can be added later over files we already know how to select, with no change to the stored format.

**Host-wide queries.** "Everything on this host in the last five minutes" is native to journald and a merge across every producer here. This is the real cost of partitioning. It is bounded by instance count rather than record count, and the operator interface does not currently offer such a query — every request must name an app or an infrastructure component. If a host-wide firehose is ever wanted, this is the design decision to revisit.
