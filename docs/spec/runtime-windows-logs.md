The Seedling Windows log store is the log sink for the [Windows Container Runtime](runtime-windows-containers.md), standing in for the system journal the Linux runtime writes to. It defines how workload output and runtime records are captured, stored, retained, and served, so that the operator interface's log surface (`i[stream.logs]`, `i[logs.entry]`, `i[logs.target]`, `i[logs.follow]`, `i[logs.tail]`) behaves the same as it does on a journald host. Rule IDs use the `wlog[...]` namespace.

This is a separable component: the container runtime spec depends on the contract stated here, not on how it is realised, so a different log engine can replace this document without disturbing the rest of the runtime.

# Producers

> wlog[producer.single-writer]
> The store is written by more than one process. Every file in it has exactly one writing process for its whole life; no file is ever appended to by two processes. All coordination between producers is therefore a read-time concern, not a locking one.

> wlog[producer.capture]
> A workload's standard output and standard error are captured by a per-instance log writer, launched and owned by the instance's container supervisor rather than by seedlingd. Capture is therefore independent of the control plane: it begins when the container starts and continues across a seedlingd stop, crash, or upgrade.

> wlog[producer.capture.config]
> The log writer is given, at launch, the identifying fields for the instance it serves and the thresholds at which it rolls a segment. It carries no policy of its own and reads no configuration: what it records and when it rolls are decided by seedlingd and passed in.

> wlog[producer.runtime-records]
> seedlingd writes its own action records — the `rt.*` call breadcrumbs and runtime events — into the same store, under the same identifying fields as the container output they relate to. These records and container output share the store precisely so that a query returns them interleaved in time order: an operator reading an app's logs sees the closure's call sequence against the output it produced.

> wlog[producer.single-path]
> Container output reaches the store by exactly one path. The runtime must not configure a second sink for the same output, so that the store cannot be used as an injection point by writing the same record twice by different routes.

# Layout

> wlog[layout.partition]
> Stored records are partitioned by the fields the operator interface filters on: app, resource, and instance for workloads, and component for infrastructure. Resolving a [target](#wlog--read.select) selects a set of partitions; no secondary index over the records is required to satisfy a filtered query.

> wlog[layout.segment]
> Each producer's records are held in a sequence of segments, numbered monotonically. A producer's segment *n* holds only records older than its segment *n+1*, so a producer's segments can be ordered, and searched by time, without opening all of them.

> wlog[layout.active]
> At most one of a producer's segments is active — the one it currently appends to. All others are sealed and receive no further writes.

# Records

> wlog[record.fields]
> Each record carries the fields the operator interface requires of a log entry (`i[logs.entry]`): timestamp, message, the producing unit, the stream it came from, and the identifying fields for its target. The unit is the instance's container identity, which is the unit of supervision on this runtime (`wcr[container.model]`).

> wlog[record.stream]
> A workload's standard output and standard error are distinguished in the record, not merged, so `i[logs.entry]`'s stream field is observed rather than inferred.

> wlog[record.sequence]
> Each record carries a sequence number, monotonic per producer. Merging orders by timestamp and breaks ties by sequence, so that a host clock step can never reorder a single producer's own records against each other.

# Reading

> wlog[read.select]
> A log request resolves its target to the matching partitions and reads only those. A request naming an app reads that app's partitions; naming a resource or instance narrows further; naming an infrastructure component reads that component's partitions.

> wlog[read.merge]
> Records from the selected partitions are delivered as a single stream ordered by timestamp, ties broken by [sequence](#wlog--record.sequence).

> wlog[read.tail]
> Historical delivery reads backwards from the newest records. The cost of serving `i[logs.tail]` must not grow with the volume of stored records: requesting the last hundred entries from a large store must not read the store from its beginning.

> wlog[read.follow]
> In follow mode, records appended after the historical cut are delivered as they are written, without the reader holding any lock that could block a producer's append.

> wlog[read.torn]
> A producer killed mid-append can leave a partial trailing record. A partial trailing record is discarded on read; it is not surfaced as an entry and does not fail the request.

# Retention

> wlog[retain.rollover]
> Only a producer rolls its own [active segment](#wlog--layout.active), sealing it and opening the next, when it crosses the size or age threshold it was given. No other process may rename, remove, or truncate an active segment: the platform does not reliably permit it while the producer holds the file open, so the operation is the producer's alone.

> wlog[retain.owner]
> seedlingd owns everything after sealing: it compresses and removes sealed segments according to retention policy, and never modifies an active one. Because the policy is applied by the control plane, no retention is applied while seedlingd is down; capture continues regardless.

> wlog[retain.compress]
> The most recent sealed segments are retained uncompressed, so that [tail](#wlog--read.tail) and [follow](#wlog--read.follow) reads never decompress. Compression is atomic: a compressed segment becomes visible only once complete, so an interrupted compression can never leave a segment readable in both forms and duplicate its records into a query.

> wlog[retain.backstop]
> A producer discards its own oldest sealed segments beyond an absolute cap, independently of the retention policy. The cap exists so that a prolonged control-plane outage bounds the store's growth rather than exhausting the host's storage and taking the workloads with it; it is set high enough that ordinary retention reclaims first.
