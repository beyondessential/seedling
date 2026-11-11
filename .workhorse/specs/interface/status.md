---
id: STS
---

# Host status

## Status

<!-- from interface: Status -->

> i[status.infra]
> `/infra/status` returns the running state of infrastructure components managed by the Seedling daemon.
> The response contains the following fields:
>
> - `proxy`: `"running"` if at least one proxy container slot is running, otherwise `"stopped"`.
> - `resolver`: `"running"` if at least one resolver container slot is running, otherwise `"stopped"`.

> i[status.ping]
> `/server/ping` is a trivial liveness probe. It accepts no params, returns an empty object `{}`, and must never fail except on transport errors. Clients use it to confirm the daemon is reachable without incurring the cost of `/server/status`.

> i[status.get]
> `/server/status` returns a summary of the running Seedling instance.
> It must always succeed and must not perform any expensive computation.
> The response contains the following fields:
>
> - `version`: the Seedling version string.
> - `hostname`: the hostname of the machine running the Seedling instance.
> - `uptime_secs`: the number of seconds since the Seedling process started.
> - `spki_fingerprint`: the SHA-256 fingerprint (hex-encoded) of the server's raw public key, allowing clients to verify or record the identity of the instance they are connected to.
> - `apps_total`: total number of registered apps.
> - `apps_by_status`: an object map of status name to count, covering only statuses with a non-zero count.
> - `active_operations`: number of lifecycle operations currently in progress.
> - `active_faults`: number of currently active faults across all apps.
> - `active_shells`: number of open shell sessions.
> - `active_forwards`: number of active port forwards.
