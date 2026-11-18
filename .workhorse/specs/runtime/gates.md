---
id: GRUN
---

# Gate runs

Applies to how the [runtime](../overview.md#runtime) keeps an installed [app](../overview.md#app)'s [gates](../language/gate.md) satisfied: when it runs and checks them, what it records of them, and how a failed run is retried.

## Terminology

### Gate run

One run of a gate as a [lifecycle operation](../overview.md#lifecycle-operation): its preparation, its body and its check.

### Gate record

The durable record the runtime keeps for each gate of an app: the gate's [last success](../language/gate.md#last-success) and whether it is still valid, the runs that have failed since, and whether the gate is paused.

### Valid success

A last success that no later run or failed check has invalidated.

### Set up

An installed app is set up once every gate it declares has been satisfied at the same time since the app was last installed.

## Gate state

At each iteration of the [reconciliation loop](reconciliation.md#reconciliation-loop), the runtime MUST determine for each gate of each installed app whether it is satisfied, unsatisfied, or undetermined.

A gate is satisfied when its gate record holds a valid success for the gate's current key.
Otherwise it is unsatisfied, unless it is undetermined.

A gate is undetermined while:

- the app's definition fails to evaluate;
- the runtime cannot read the gate's record;
- its record holds a valid success for its current key, and its most recent check could not be run, as [checks](#checks) specify.

The runtime MUST NOT treat an undetermined gate as satisfied or as unsatisfied.
It MUST NOT run an undetermined gate, nor start a dependant waiting on it, and MUST NOT stop anything because of it.

While it cannot read a gate's record, the runtime MUST hold a `gate_state_unreadable` [fault](faults.md#app-faults) against the gate, and MUST keep trying to read it.

## Initiating runs

The runtime MUST initiate a run of a gate of an installed app when the gate is unsatisfied, is not [paused](#pauses), has no run in progress or queued, each of its [targets](../language/gate.md#start-order) is ready, and its [retry](#retries) is due.
The run has the trigger `gate`, and runs against the generation current when it is dispatched, as [initiating events](operations.md#initiating-events) specify.

Where several gates of an app are due at once, the runtime MUST initiate them in the order the script declared them.

A gate run is subject to the [concurrency](operations.md#concurrency) rules of every lifecycle operation.
Where those rules do not let the runtime queue a run, the run is not initiated and has not failed, and the runtime MUST initiate it on a later iteration once they do.

## Preparation

When a run starts, the runtime MUST first invalidate the gate's last success in its record.
If it cannot, the run MUST fail with nothing stopped.

> [!NOTE]
> A run that an operator starts for a satisfied gate leaves it unsatisfied until a run succeeds, so a migration that fails part way keeps the selection stopped.

Before it stops anything for a run, the runtime MUST pull every image that the run could need:

- the images the gate's body might pull, as an [image discovery](images.md#image-discovery) probe of the body at the run's generation reports them;
- the image of the gate's check;
- the images of the gate's dependant workloads at the run's generation.

If a pull fails, the run MUST fail with nothing stopped, and the instances that were running MUST go on running.

The runtime MUST then stop the gate's [exclusive selection](../language/gate.md#exclusive-selection), wait until every instance of it has reached Terminated, and only then run the body.
How the selection is held from then on is specified by [gate holds](reconciliation.md#gate-holds).

## Outcome

A run succeeds when its body returns and, where the gate declares a check, the check then passes.
A run whose body throws, whose check then fails or cannot be run, or that is cancelled, does not succeed.

When a run succeeds, the runtime MUST write the gate's record, holding a digest of the gate's key at the run's generation and that generation as a valid success, before it counts the gate as satisfied.
If it cannot write the record, the run MUST count as failed.

The runtime MUST compute the digest under the host's [secret key](secrets.md#secret-key), and MUST NOT store in a gate's record the key itself, nor anything from which the key could be recovered without the secret key.

> [!NOTE]
> A key can carry a secret, such as a password a gate rotates.
> A record keyed on a digest under the secret key tells keys apart without holding them.

A run's success makes the gate satisfied only for the key of the run's generation.
Where the app's current key for the gate differs by then, the gate is unsatisfied, and the runtime MUST initiate another run as [initiating runs](#initiating-runs) specifies.

When an interrupted gate run is not [replayed](operations.md#interrupted-operations), the gate stays unsatisfied, and the runtime runs it again.

## Retries

The runtime MUST record each run that does not succeed in the gate's record, with when it ended and how.

The runtime MUST space consecutive failed runs of a gate by a delay that grows with each failure, up to a bounded maximum.
The runtime SHOULD begin that delay at 30 seconds, double it with each consecutive failure, and cap it at one hour.
However long a gate's runs have been failing, the runtime MUST run it again once the maximum delay has passed since the last failure.
A successful run MUST reset the delay.
A move of the gate's key MUST make its next run due at once.

While a gate is unsatisfied and its most recent run did not succeed, the runtime MUST hold a `gate_failed` [fault](faults.md#app-faults) against the gate, carrying how the run ended.

## Pauses

When a gate's run is cancelled, the runtime MUST record in the gate's record that the gate is paused, for the key of the cancelled run's generation.
The runtime MUST NOT initiate a run of a paused gate.
A gate stays paused until an operator [runs it](../interface/app-control.md#gate-runs), or its key moves from the key it was paused for.

## Checks

The runtime MUST check a gate that declares a check:

- before it starts an instance of a dependant workload while no instance of that workload is running, where the gate's record holds a valid success for its current key;
- after the gate's body returns in a run;
- every `interval` seconds while the app is installed and no run of the gate is in progress, where the check declares an interval.

To check a gate, the runtime MUST start an instance of the check's Job and wait for it to terminate.
The Job's definition is that of the run's generation for the check that ends a run, and that of the app's current generation otherwise.
The runtime MUST run a check whatever other lifecycle operation is in progress.

A check that passes leaves the gate as its record says.
A check that fails MUST invalidate the gate's last success in its record, so that the gate is unsatisfied until a run succeeds.
If the runtime cannot record the failure, the gate is undetermined.

A check could not be run when the runtime could not start its Job's instance.
The runtime MUST then hold a `gate_check_unavailable` [fault](faults.md#app-faults) against the gate until a check of it next runs to an outcome, and MUST check it again with a delay that grows as for [retries](#retries).

## Cancellation

An operator cancels a gate run as any lifecycle operation, as [cancellation](operations.md#cancellation) specifies.
A cancel requested while the run pulls images, waits for its exclusive selection to stop, or runs its check MUST end the run at once, stopping the check's Job where it runs.

## Gates no longer declared

When the app's definition evaluates and does not declare a gate that it declared before, the runtime MUST withdraw any queued run of the gate, and MUST clear its faults.
A run of the gate already in progress goes on against its generation, and the runtime records its outcome as for any run.

The runtime MUST keep the gate's record until the app is uninstalled, so that a gate declared again under the same name resumes from it.

> [!NOTE]
> A renamed gate is a new gate, which runs as for the first time, with `last` the unit value.

## App setup

The runtime MUST record durably that an installed app is set up the first time it finds every gate the app declares satisfied at once.
An app that declares no gates is set up once it is installed.

## Uninstall

When an app's [uninstall](app-teardown.md) begins, the runtime MUST delete the records of all its gates, the records of their [holds](reconciliation.md#gate-holds), and the record that the app is set up, so that installing it again runs every gate as for the first time.
While it cannot delete them, the runtime MUST keep trying, and MUST NOT move the app to `NotInstalled`.
