---
id: GRUN
---

# Gate runs

Applies to how the [runtime](../overview.md#runtime) keeps an installed [app](../overview.md#app)'s [gates](../language/gate.md) satisfied: when it runs and checks them, what it records of them, and how a failed run is retried.

## Terminology

### Gate run

One run of a gate as a [lifecycle operation](../overview.md#lifecycle-operation): its preparation, its body and its check.

### Gate record

The durable record of a gate's [last success](../language/gate.md#last-success).

## Gate state

At each iteration of the [reconciliation loop](reconciliation.md#reconciliation-loop), the runtime MUST determine for each gate of each installed app whether it is satisfied, unsatisfied, or undetermined.

A gate is undetermined while:

- the app's definition fails to evaluate;
- the runtime cannot read the gate's record;
- the gate's check could not be run, as [checks](#checks) specify.

The runtime MUST NOT treat an undetermined gate as satisfied or as unsatisfied.
It MUST NOT run an undetermined gate, nor start a dependant waiting on it, and MUST NOT stop anything because of it.

While it cannot read a gate's record, the runtime MUST hold a `gate_state_unreadable` [fault](faults.md#app-faults) against the gate, and MUST keep trying to read it.

## Initiating runs

The runtime MUST initiate a run of a gate of an installed app when the gate is unsatisfied, the gate has no run in progress or queued, each of the gate's [targets](../language/gate.md#start-order) is ready, and the gate's [retry](#retries) is due.
The run has the trigger `gate`, and its source and target generations are both the app's current generation when the run is initiated.

Where several gates of an app are due at once, the runtime MUST initiate them in the order `after` gives them, and otherwise in the order the script declared them.

A gate run is subject to the [concurrency](operations.md#concurrency) rules of every lifecycle operation, and queues as they specify.

## Preparation

Before it stops anything for a run, the runtime MUST pull every image that the run could need:

- the images the gate's body might pull, as an [image discovery](images.md#image-discovery) probe of the body at the run's target generation reports them;
- the image of the gate's check;
- the images of the gate's dependant workloads at the run's target generation.

If a pull fails, the run MUST fail with nothing stopped, and the instances that were running MUST go on running.

The runtime MUST then stop the gate's [exclusive selection](../language/gate.md#exclusive-selection), wait until every instance of it has reached Terminated, and only then run the body.
How the selection is held from then on is specified by [gate holds](reconciliation.md#gate-holds).

## Outcome

A run succeeds when its body returns and, where the gate declares a check, the check then passes.
A run whose body throws, whose check then fails or cannot be run, or that is cancelled, does not succeed.

When a run succeeds, the runtime MUST write the gate's record, holding the key of the run's target generation and that generation, before it counts the gate as satisfied.
If it cannot write the record, the run MUST count as failed.

The runtime MUST NOT store a gate's key in its record, nor anything from which a key could be recovered without the host's [secret key](secrets.md#secret-key).

> [!NOTE]
> A key can carry a secret, such as a password a gate rotates.
> A record keyed on a digest under the secret key tells keys apart without holding them.

A run's success makes the gate satisfied only for the key of the run's target generation.
Where the app's current key for the gate differs by then, the gate is unsatisfied, and the runtime MUST initiate another run as [initiating runs](#initiating-runs) specifies.

When an interrupted gate run is not [replayed](operations.md#interrupted-operations), the gate stays unsatisfied, and the runtime runs it again.

## Retries

The runtime MUST space consecutive failed runs of a gate by a delay that grows with each failure, up to a bounded maximum.
The runtime SHOULD begin that delay at 30 seconds, double it with each consecutive failure, and cap it at one hour.
However long a gate's runs have been failing, the runtime MUST run it again once the maximum delay has passed since the last failure.
A successful run MUST reset the delay.

When a gate's run is cancelled, the runtime MUST NOT run the gate again until an operator [runs it](../interface/app-control.md#gate-runs) or its key changes.

When a run of a gate fails, the runtime MUST file a `gate_failed` [fault](faults.md#app-faults) against the gate, carrying the failure.

## Checks

The runtime MUST check a gate that declares a check:

- before it starts an instance of a dependant workload while no instance of that workload is running;
- after the gate's body returns in a run;
- every `interval` seconds while the app is installed, where the check declares an interval.

To check a gate, the runtime MUST start an instance of the check's Job with its definition at the app's current generation, and wait for it to terminate.

A check that passes leaves the gate as its record says.
A check that fails makes the gate unsatisfied, whatever its record says.

A check that could not be run, because its Job's instance could not be started or did not terminate, makes the gate undetermined.
The runtime MUST then file a `gate_check_unavailable` [fault](faults.md#app-faults) against the gate, and MUST check it again with a delay that grows as for [retries](#retries).

## Uninstall

When an app's [uninstall](app-teardown.md) begins, the runtime MUST delete the records of all its gates, and every record that one of its gates [holds](reconciliation.md#gate-holds) its exclusive selection, so that installing it again runs every gate as for the first time.
