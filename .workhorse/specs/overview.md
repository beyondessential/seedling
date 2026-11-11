---
id: SEED
---

# Seedling

Seedling runs containerised apps on a single host, each app defined by a script and kept running by a daemon that operators control remotely.

## Requirements notation

The key words MUST, MUST NOT, REQUIRED, SHALL, SHALL NOT, SHOULD, SHOULD NOT, RECOMMENDED, NOT RECOMMENDED, MAY and OPTIONAL in Seedling's specifications are to be interpreted as described in BCP 14 ([RFC 2119](https://www.rfc-editor.org/rfc/rfc2119), [RFC 8174](https://www.rfc-editor.org/rfc/rfc8174)) when, and only when, they appear in all capitals, as here.

## External documents

| document | what it covers |
| --- | --- |
| [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119) and [RFC 8174](https://www.rfc-editor.org/rfc/rfc8174) | the requirement keywords above |
| [Rhai](https://rhai.rs/book/) | the scripting language BSL is written in, under [BSL](language/overview.md) |

## Terminology

### Operator

A person or automated system that manages a host's apps through the [Operator Interface](#operator-interface).

### Host

The machine on which one Seedling daemon runs and manages apps.

### Runtime

The part of the Seedling daemon that evaluates app definitions, observes the host, and acts on it.

### Operator Interface

The network interface through which operators observe and control a host.
Defined in [OI](interface/overview.md).

### App

A named unit of software installed on a host, defined by one definition bundle.
Defined in [APP](language/app.md).

### Definition bundle

The tree of files, including one or more script files, that defines an app.
Defined in [BND](language/bundle.md).

### Resource

A single declared thing an app is made of, such as a deployment, service, ingress or volume.
Defined in [BSL](language/overview.md).

### Resource graph

The set of resources produced by evaluating an app's definition with its parameter values.
Defined in [BSL](language/overview.md).

### Generation

A number identifying one state of an app's definition and parameter values.
Defined in [GEN](runtime/generation.md).

### Desired state

The resources that should exist for an app, each with its intended lifecycle state.
Defined in [RCN](runtime/reconciliation.md).

### Lifecycle state

The stage a resource instance has reached between being wanted and being cleaned up.
Defined in [LCY](runtime/lifecycle.md).

### Lifecycle operation

A single run of an action closure directing the runtime through a change to an app.
Defined in [OPS](runtime/operations.md).

### Fault

A record of a condition on a host that needs an operator's attention.
Defined in [FLT](runtime/faults.md).
