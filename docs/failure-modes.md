# Failure modes

Recurring ways this codebase has gone wrong, and the rule that prevents each.

The first eight came from a whole-codebase logic-bug audit, which found the same handful of
mistakes repeated across unrelated subsystems. They are grouped by the shape of the mistake
rather than by subsystem, because that is how they recur: the next instance will be in a
file none of these examples touch.

Each entry gives the rule, the concrete failure that motivated it, and what to look for
while writing or reviewing. Where a rule is mechanically checkable there is a script under
`etc/ci/` and a spec item under `docs/spec/`.

Add to this document when a mistake turns out to be a class rather than a one-off — when
the same shape shows up in a second unrelated subsystem, or when a fix for one instance
reintroduces it somewhere else. A rule earns its place by naming what to look for, not by
recounting what went wrong.

---

## 1. "Could not determine" is not an answer

**Never let a failed query, an unreadable row, an unparseable value, or a skipped action
become a definite result.**

This is the most repeated defect of the lot, and the one that most readily reappears *in the
fixes for it*. Every instance looks locally reasonable and is destructive in aggregate:

- A failed podman/systemd probe returned an observation with every flag false — byte-for-byte
  what "confirmed absent" looks like. The Job terminal-detection predicate read that as
  "naturally finished", stopped an in-flight batch workload and recorded it as completed, from
  which `job-terminal.defense` guarantees it is killed again if it reappears.
- An unrecognised container state mapped to "container missing" *in an arm that had already
  proven the container exists*, so a container draining through its stop timeout was recorded
  as removed and barriers over its volumes released early.
- A failed database read produced an empty expected-unit set, which filtered every unit away
  and read as "teardown finished", deleting the registry rows while the units were still loaded.
- A failed ownership lookup was treated as "not owned" by a destructive startup sweep.
- A withheld apply returned `Ok(())`, which the caller could not distinguish from a successful
  one, so it *cleared* the fault for the thing it had deliberately not done.

**Check:** for every error branch, ask what the caller will conclude from the value you return.
If failure and a definite negative produce the same value, the type is wrong — split them
(`Observed`/`Failed`, `Some(Result)`/`None`) so the compiler forces the caller to decide.
Absence *confirmed by a successful query* is real evidence and must keep working; it is only
the unknown case that must not masquerade as it.

Spec: `r[observe.failure-not-absence]`, `r[reconciliation.absolute-state]`.

## 2. On failure, observable state is unchanged

**A failure path must leave the world as it found it, or as the last good state.**

`evaluate_script` returns an `App` populated up to wherever the script threw. That contract is
right for registration, where no previous definition exists, and was applied unchanged to
reload — so a typo halfway down a script published a truncated definition, and the post-update
diffs read it as "the operator deleted everything below this line": volume data relocated,
scaling decisions wiped, forwards torn down, schedules pruned. One `throw`.

**Check:**

- A function returning a value alongside an error (`(T, Option<E>)`): may that `T` ever replace
  a previous good one? If the answer differs by caller, return an enum, not a tuple.
- A handler that mutates memory and the database: commit durably *before* the change becomes
  observable, and make sure every early `return Err` leaves both untouched. A half-registered
  app that `/apps/list` shows and a restart drops is worse than a clean failure.
- State that is rebuilt in full and applied wholesale — routes, nftables rules, proxy config —
  must not be applied when a contributor is missing from it. Absent is indistinguishable from
  deleted, and the containers are still running.

Spec: `i[app.update]`, `i[app.register]`, `r[reconciliation.absolute-state]`.

## 3. Match identities against the record, not the shape of a name

**`starts_with`, or SQL `LIKE 'x%'`, on a unit, container, network, volume or ingress name is
a red flag.**

Uninstall recognised an app's units with `seedling-{app}-`. Both app names and resource names
may contain hyphens, so the encoding is not prefix-free: uninstalling `app` matched every unit
of a sibling called `app-db` and stopped them, every tick, while never completing. The exact
identities were already in `resource_instances` the whole time.

A prefix scan is fine to *enumerate candidates*; the decision must be an exact match against
recorded identity.

The same discipline at grant time: names the daemon takes in an operator-shared namespace
(`backup-snap-*` volumes, the `tailscale` ingress) belong in `crates/core/src/reserved.rs`,
are rejected **at creation only** — an operator must still be able to rename or remove
something that predates the reservation — and their destructive consumers additionally check
recorded ownership, because reservation cannot repair a collision that already exists.

Derivation is not allocation. Deriving a pod subnet from eight bits of an instance id gave
every static Job the same one, because their ids are nil. If uniqueness matters, allocate and
record the allocation.

Spec: `r[app.uninstall.scope]`, `r[namespace.reserved]`, `r[infra.pod.subnet]`.

## 4. Every fault names both halves of its lifecycle

**A fault kind that does not say when it clears is not finished.**

Dedup, subject identity, and the clearing condition were re-implemented at every call site and
had diverged into five idioms. The results: a fault filed once per event without bound and
never cleared; faults that could never clear after a restart because the "previously seen" set
was in memory; and a successful backup of one volume clearing every *other* volume's failure.

**Check:**

- Identity is `(app, kind, subject)` — the subject is the *thing* that is faulty, never a
  substring of the description. File through `file_once` or `sync_faults`.
- **Clear no more broadly than you file.** A kind-wide clear for a subject-keyed fault is
  exactly the bug above.
- Say which shape it is: a *condition* fault (true right now — converge it against the
  database each tick, which is restart-safe by construction), an *event* fault (this happened —
  dedup on file, and name the clear path explicitly), or *latched* (deliberately outlives its
  trigger — name the lifecycle event that clears it).
- The file set and the clear set for one tick must be disjoint per key.

Spec: `r[fault.lifecycle]`.

## 5. Every retry needs a back-off and a way back

**A loop containing a fallible await names three things: its back-off, its transient/fatal
classification, and its exit-reporting path.**

Every site that got this wrong named at most one. Image pulls retried on every 5 s tick and
then set an `exhausted` flag nothing could clear, because entries were removed only on a
success that could no longer be attempted — a briefly-unreachable registry disabled the
workload until the daemon restarted. A UDP relay treated a legal zero-length datagram and an
ICMP port-unreachable as fatal and exited silently, leaving the forward listed as healthy
with nothing behind it.

**Check:**

- No terminal give-up state without an expiry or an operator reset path. Past a threshold,
  escalate to a fault and keep attempting at the cap — the fault can only clear because you
  kept trying.
- `"retry immediately"` and a bare `_ => break` are the tells.
- Both ends of one protocol must classify errors identically. They did not, and one end killed
  forwards the other end merely dropped a datagram on.
- **A subsystem with a central decision function admits no dispatch before the decision.** TLS
  issuance dispatched one provider ahead of the state machine and so ignored the operator block
  and the failure debounce — and the resulting attempt-row flood evicted *other* hostnames'
  debounce state.

Use `runtime::retry::RetryGate` for tick-driven pacing rather than hand-rolling counters.
Spec: `r[actuate.image.retry]`, `i[forward.relay.resilience]`.

## 6. Throw at the boundary; never coerce

**A value crossing the BSL boundary is converted with `defs::take::*` or an equivalent throw.**

`into_string().unwrap_or_default()`, `filter_map(try_cast)` and bare `as` casts do not fail on
a script type error — they change what the script *means*, far from the cause.
`select(#{ types: ResourceType.Service })` with the brackets forgotten dropped the criterion,
and a `select` with no surviving criteria matches every resource in the app, so a following
`rt.stop` stopped every workload. `pids_limit(4294967297)` wrapped to 1.

Genuine type *dispatch* that ends in a throw is fine; silent defaulting is not.

Enforced by `etc/ci/check-defs-coercion.sh`. Spec: `l[bsl.args.strict]`.

## 7. Anything persisted for crash recovery answers four questions

**Who else writes this row? Which restart path reads it? What happens on every abort branch of
that path? Which test severs in-memory state between the write and the read?**

If the last question has no answer, the change is not done — every finding in this class had a
write path and a read path that were each tested alone.

- **Replay is positional.** A replayed call matches the committed entry at *its own position*,
  never a similar-looking one elsewhere. Scanning by value swallowed a second identical call
  and re-delivered one whose arguments had shifted. Validate arguments that are stable by
  construction (a literal in the script); do not validate ones that legitimately vary between
  passes (a resolved instance set).
- **Never `INSERT OR REPLACE` a row another writer touches.** It is delete-then-insert: every
  column you do not name reverts to its default. It was correct when written and became wrong
  when a migration added `cancel_requested`, silently dropping cancellations. Use
  `ON CONFLICT ... DO UPDATE` naming your own columns, so a future `ALTER TABLE ADD COLUMN` is
  safe by default.
- **Stamp an effect when it happens, not when it is intended.** A queued schedule fire was
  recorded as fired while it existed only in an in-memory queue, so a restart lost it and the
  catch-up guarantee had nothing to catch up on.

Enforced by `etc/ci/check-insert-or-replace.sh`. Spec: `r[barrier.replay.positional]`,
`r[history.persist.partial-update]`.

## 8. A wire contract belongs in one place

**When several call sites implement the same protocol handshake, that is the bug.**

Four consumers hand-rolled the subscription handshake and each got a different part wrong: two
parked forever on an error response, one reported a rejection as success. The happy path works
on first test and the error path is invisible until a transient `server_busy` bricks a
long-lived consumer.

The per-caller variation is what happens *after* — retry policy, exit code, how the error is
rendered. The handshake itself is not per-caller.

Enforced by `etc/ci/check-accept-uni.sh`. Spec: `i[stream.subscribe]`.
