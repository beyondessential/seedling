Here's my review of the BSL specification in `language.md`, identifying areas that appear missing or incomplete for the spec to be considered complete. I've organized them from most significant to least.

## 1. Runtime Instance (`lang.rt`) — Referenced but never defined

The spec references `rt` (the "Runtime Instance") extensively across actions:

```beset/docs/spec/language.md#L217-L218
> The `fn` closure may take one argument, the [Runtime Instance](#r--lang.rt), typically named `rt`. Specialised Actions may have access to more arguments.
```

Methods like `rt.start(app)`, `rt.stop(old)`, and `rt.action(app, "start")` are used in default implementations, but there is **no `## Runtime Instance` section** and **no `r[lang.rt]` requirement** defining:
- What methods `rt` exposes (`start`, `stop`, `action`, and likely more)
- What `rt.start(app)` actually does (start all deployments? in what order?)
- What `rt.stop(old)` does
- What `rt.action(app, "start")` does (invoke another action by name?)
- Whether `rt` has other capabilities (logging, waiting, health checks, etc.)

This is the single largest gap — actions are the "verbs" of the language, and the runtime is how they act on the world.

## 2. Application History (`lang.history`) — Referenced but never defined

The crash recovery action references it:

```beset/docs/spec/language.md#L263-L264
> Its `fn` closure may take up to two arguments: the [Runtime Instance](#r--lang.rt) (typically named `rt`) and the [Application History](#r--lang.history) (typically named `history`).
```

There is no `r[lang.history]` requirement defining what the history object contains or what methods it exposes.

## 3. Only one phase defined

The phases section declares:

```beset/docs/spec/language.md#L16-L19
> A BSL script is executed several times, in different phases.
>
> - `dphase`: Definition Phase. In this phase, static resource definitions are created by the BSL, parameter names are collected, and actions are registered. Many methods and functions will return special [placeholder values](#r--lang.placeholder) during this phase, as the real values are not yet known.
```

Only **definition phase** is listed, but the spec says "several times, in different phases" (plural). What are the other phases? An "execution phase" or "action phase" is implied by the existence of actions and runtime instances, but never specified. This matters because the behavior of many functions changes by phase (e.g., `app.param()` returns placeholders in dphase — what does it return in other phases?).

## 4. Volume type — never defined

`r[lang.container.mount-volume]` references `Volume` and `r[lang.external-volume]` returns a `Volume`, but:
- There is no `r[lang.volume]` defining the `Volume` type
- Can volumes be created within BSL (e.g., ephemeral/scratch volumes), or only obtained externally?
- What properties does a `Volume` have?

## 5. Error handling and validation semantics

The spec describes what arguments "must" be (e.g., port must be non-zero, prefix must start with `/`), but doesn't specify:
- What happens when validation fails — does the script abort? Is an error value returned? Is an exception thrown?
- How Rhai exceptions interact with BSL (Rhai has `try`/`catch`)
- Whether errors in definition phase vs. execution phase behave differently

## 6. Deployment lifecycle and state machine

The spec defines what a Deployment *is* and its update strategies, but doesn't cover:
- **Health checks / readiness** — `r[lang.deployment.strategy.rolling]` mentions waiting until a container "becomes ready," but there's no requirement for how readiness is defined or configured
- **Deployment states** — running, stopped, updating, failed, etc.
- **What happens when a container crashes** during normal operation (not node crash) — restart policy?

## 7. Job lifecycle

`r[lang.job]` says a Job is "short-lived, one-off" and implements Container, but:
- How is a Job started? (Only via `rt` in an action?)
- What happens when a Job finishes (success vs. failure)?
- Can a Job's exit code / output be observed?
- Is there a timeout mechanism?

## 8. Service discovery and networking model

- How do pods/containers discover services? Is it always `localhost` via `pod.mount(svc)`?
- Can services communicate across apps?
- What happens when a service has no backing pod (e.g., during an upgrade with `Replace` strategy)?

## 9. Resource naming constraints

Several methods take `name: string` (`app.deployment(name)`, `app.service(name)`, `app.job(name)`, etc.) but the spec never defines:
- What characters are valid in names
- Maximum length
- Whether names must be unique across types or only within a type
- Case sensitivity

## 10. ~~`app` — missing methods inventory~~

The `App` type is referenced with many methods scattered throughout (`app.param()`, `app.service()`, `app.deployment()`, `app.job()`, `app.external_volume()`, `app.on_action()`, `app.on_start()`, `app.on_upgrade()`, `app.on_crash_recovery()`, `app.on_shell()`, `app.on_install()`), but there's no single requirement enumerating the full API surface. It's unclear whether this is the exhaustive list.

## 11. Ingress — incomplete builder

`r[lang.ingress]` defines `.host()` and `.tls()` but:
- Is `.host()` required or optional?
- Can an ingress have multiple hosts?
- What happens without TLS — is plain HTTP served?
- How does path-based routing interact with the HTTP service route prefixes?
- Is there any way to configure HTTP→HTTPS redirect behavior?

## 12. Deployment `scale` — missing definition phase behavior

Several types document their dphase behavior (`r[lang.param.dphase]`, `r[lang.external-volume.dphase]`), but `Deployment`, `Service`, `Job`, `Container`, `Pod`, `Ingress`, and `Action` have no dphase requirements. Are builders valid in all phases? Presumably definitions only happen in dphase, but this is unspecified.

## 13. The `old` app in Upgrade Action

```beset/docs/spec/language.md#L243-L244
> Its `fn` closure may take up to two arguments: the [Runtime Instance](#r--lang.rt) (typically named `rt`) and the `App` instance being replaced (typically named `old`).
```

What can you do with the `old` app? Is it the same `App` type? Can you inspect its parameters, deployments, services? Can you diff it against the new app?

## 14. Concurrency and ordering

- If multiple deployments are defined, are they started in order or concurrently?
- Can actions run concurrently?
- Is BSL single-threaded (Rhai is)?

## 15. Missing built-in functions / global scope

Beyond `app`, `DeploymentStrategy`, and the types returned by builders, what else is in the global scope? Are there utility functions, logging, sleep/delay, or assertion capabilities?

---

**Summary:** The biggest gaps are the **Runtime Instance** (`rt`), the **remaining execution phases**, the **Volume type**, and the **Application History** — these are all referenced but undefined. After those, the lifecycle semantics (health checks, restart policies, error handling) and the networking/discovery model are the next most important areas to pin down.
