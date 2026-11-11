---
id: RUN
---

# Runtime

## Preamble

<!-- from runtime: None -->

The Seedling Runtime is the component responsible for making the real world match what a BSL script declares.

It evaluates BSL scripts, observes the state of the system, and continuously reconciles the two.
It also executes action closures defined in BSL, which direct the runtime through lifecycle transitions such as starting, upgrading, and installing an application.

Absent specification bugs, anything that is not defined here is either defined in another spec document (the language spec, the operator interface spec), or is implicitly not allowed.
