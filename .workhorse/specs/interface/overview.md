---
id: OI
---

# Operator Interface

## Preamble

<!-- from interface: None -->

The Seedling Operator Interface (OI) is the channel through which external actors observe and control a running Seedling instance.

Its consumers are human operators (via CLI or UI), agentic operators, and automation pipelines.
The OI is the exclusive mechanism for registering applications, transitioning them out of the uninstalled state, invoking lifecycle actions, changing parameter values, opening interactive shell sessions, and receiving the fault and event feed.

Absent specification bugs, anything that is not defined here is either defined in another spec document (the language spec, the runtime spec), or is implicitly not allowed.
