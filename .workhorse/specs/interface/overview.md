---
id: OI
---

# Operator Interface

Applies to the [Operator Interface](../overview.md#operator-interface) as a whole: the roles at its two ends and the reach of its authority over a [host](../overview.md#host).

## Terminology

### Server

The endpoint of the OI that the Seedling daemon on a host serves.

### Client

A program that connects to a server to observe or control the host on behalf of an [operator](../overview.md#operator).

### Method

A named request a client makes of a server, such as `/apps/show`.

## Operator direction

While the daemon runs, the [runtime](../overview.md#runtime) MUST accept direction from operators only through the OI.
Direction includes registering, installing, uninstalling and removing [apps](../overview.md#app), invoking lifecycle actions, changing parameter values, and opening shell sessions.

> [!NOTE]
> The runtime also acts without being asked, as other specifications direct, for example when an app declares a schedule for an action.
> Configuration the daemon reads as it starts, such as its listen addresses and the bootstrap file of authorised keys in [TRN](transport.md), is not direction in this sense.
