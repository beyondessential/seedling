---
id: JOB
---

# Jobs

## Job

<!-- from language: Job -->

> l[job.type]
> A Job is a short-lived, one-off instance of a container workload.
>
> Jobs are defined using the `app.job(name: string)` method, which returns a [builder](rule:l/bsl.builder).
>
> A Job defined in the **static scope** (top level of the BSL script, outside any action or shell closure) is part of the application's steady state and will be included when `rt.start(app)` is called. A static Job has a single, fixed all-zero instance ID; only one instance of it may exist at a time.
>
> A Job defined in a **dynamic scope** (inside an action or shell closure) is not part of steady state. Each invocation of the enclosing action creates a new instance with an identity derived from that action invocation, so that two concurrent invocations of the same action can each run the Job without collision. See the runtime spec [Job instance identity](rule:r/identity.job) for the derivation.
>
> A Job passed to a shell `attach()` call receives a fresh randomly-generated instance ID, allowing multiple concurrent shell sessions against the same Job definition.

> l[job.pod]
> Job implements the [Pod](rule:l/pod.interface) interface.

> l[job.deadline]
> The `job.deadline(seconds: number)` builder method specifies how long the job can run for until it is terminated.
>
> This starts counting from when the Job enters the `Running` state.
>
> If there is no deadline, the job runs indefinitely.
