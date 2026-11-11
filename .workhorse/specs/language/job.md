---
id: JOB
---

# Jobs

Applies to the BSL `Job` resource, a container workload that runs once to completion.

## Terminology

### Job

A [resource](../overview.md#resource) whose instances each run one container workload until it exits.

## Declaration

A script declares a Job with `app.job(name: string)`, which returns a `Job` [builder](overview.md#builders).
A `Job` implements the [Pod](pod.md) interface, and so the [Container](container.md) interface.

## Instances

Apart from instances started by `shell.attach`, a [static](app.md#static-resource) Job MUST have at most one instance at a time.

Each run of an action closure that creates an anonymous Job MUST create its own instance of that Job.
Two concurrent runs of the same action closure MUST NOT share or collide on an instance.

Each call to [`shell.attach`](action.md#shell-control) MUST start its own instance of the Job it attaches to, so that several shell sessions may run against one Job at once.

How the runtime identifies each instance is specified by [Job instance identity](../runtime/identity.md#job-instances) and [shell Job identity](../runtime/identity.md#job-instances).

## Deadline

`job.deadline(seconds: number)` sets how long a Job instance may run.
A `seconds` below 1 MUST throw.

The runtime MUST stop a Job instance that has been [Running](../overview.md#lifecycle-state) for `seconds` seconds.
A Job on which `deadline` is never called runs until its container exits.
