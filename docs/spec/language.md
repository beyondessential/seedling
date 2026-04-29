BES Seedling Language, or BSL for short, is a DSL (Domain Specific Language).
It is used to define and manage an application running on a Seedling node in an autonomous way and provide administrative controls to operators.

The terminology used in BSL closely resembles that used for [Kubernetes](https://kubernetes.io), but some of the semantics are different.

Absent specification bugs, anything that is not defined here is either defined in another spec document (e.g. the control plane, the runtime), or is implicitly not allowed.

> l[bsl.syntax]
> BSL is written in [Rhai](https://rhai.rs).

> l[bsl.script]
> A BSL script is a single code listing that defines a Seedling Application.

> l[bsl.scope]
> The runtime must use a distinct [Rhai Scope](https://rhai.rs/book/engine/scope.html) for each BSL script.

> l[bsl.enums]
> Enums are not a native Rhai feature. In BSL, enums are defined as being two things simultaneously at the same name:
>
> - an opaque type, used to describe the enum in type signatures
> - a constant object map, with its keys being the names of the enum variants, and its values being opaque values of the opaque enum type.
>
> All enums available to BSL are defined in the [Constants](#constants) section.

> l[bsl.errors]
> Some methods throw exceptions under some circumstances.
> The `try..catch` Rhai construct may be used to handle those exceptions and recover.
> If an exception bubbles to the top of the script, execution is considered failed and will not proceed further.
> Responding to this is a control plane concern and not defined in this spec.

> l[bsl.builder]
> Some methods and functions return "builders", which are types that have further methods which configure one instance of the type piece by piece, rather than all at once.
>
> They have **builder methods** which modify the instance and return the builder for chaining, and may have **instance methods** which create different types from the builder state.

> l[bsl.name]
> Various methods and resources are defined using a `name`.
>
> Unless otherwise specified, names are ASCII alphanumeric with hyphens, must not start with a number, must not start nor end with a hyphen, and must be between 3 and 63 characters long inclusive.
> Names are case-sensitive.
>
> The regular expression `^[a-zA-Z][a-zA-Z0-9-]{1,60}[a-zA-Z0-9]$` may be used to validate a name.

> l[bsl.port]
> Various methods and resources use port numbers.
>
> Unless otherwise specified, a port number must be a non-zero positive integer below 65536.
> If an invalid port is provided, the method must throw.

> l[bsl.resource]
> The term Resource is using a similar definition [as Kubernetes](https://kubernetes.io/docs/reference/using-api/api-concepts/#standard-api-terminology):
> - A _resource type_ is the name used in the spec (Service, Deployment, Job)
> - A list of instances of a resource type is known as a _collection_
> - A single instance of a resource type is called a _resource_

> l[bsl.resource.description]
> Every Resource has a `description(text: string)` builder method which attaches a free-form human-readable description to the resource. The description is surfaced to operators in the management interfaces (CLI, web UI, OI), and is particularly useful for naming the purpose of anonymous resources (such as anonymous jobs spawned inside an action closure) so they are recognisable in operation logs.
>
> The description is purely informational and has no behavioural effect. Calling `description()` more than once on the same resource replaces the previous value. The description is `None` by default.
>
> The method returns the same resource builder so calls may be chained.

# Collection

> l[collection.interface]
> A `Collection` is an abstract interface of things that can be zero or more Resources.
> Workload control methods often operate on or with Collections.
> Collections can hold different resource types, and can hold Collections.
> Order within a collection is not defined.
>
> An array of Collections is a Collection of the contents.
>
> All Resources are a Collection of the resource itself and all resources that are contained (not references) in it.

> l[collection.one]
> `col.one()` is a method which returns any one Resource from the collection (or null if the collection is empty).
>
> This is most useful for collections which have zero or one resources within.

> l[collection.only]
> `col.only(other: Collection)` is a method which returns a Collection of all resources in this Collection that are present in the `other` collection.

> l[collection.except]
> `col.except(other: Collection)` is a method which returns a Collection of all resources in this Collection that are _not_ present in the `other` collection.

> l[collection.select]
> `col.select(criterion: object)` is a method which returns a Collection of all the resources within this Collection that match the `criterion`.
>
> The criterion is an object map where all keys are optional. Resources must match all keys to be selected. All possible keys are defined in this spec.

> l[collection.select.types]
> `types`: must match one of an array of [ResourceTypes](#l--const.resource-type.enum).

> l[collection.select.names]
> `names`: must match one of an array of resource names

> l[collection.select.name-patterns]
> `name_patterns`: must match at least one glob pattern

> l[collection.col]
> `col(val)` is a free function that coerces any value into a `Collection`.
>
> The following values are accepted:
> - A `Collection` — returned as-is.
> - An `App` — yields a Collection of all its named resources and actions.
> - Any named Resource (Deployment, Service, HttpService, Job, Ingress, named Volume, ExternalVolume, Action) — yields a Collection of that single resource.
> - An array — yields a Collection of all elements coerced the same way (a union).
> - An anonymous Volume (without a name) — yields an empty Collection.
> - Any other value — yields an empty Collection.

# Constants

These are not guaranteed to be constant forever, only for the duration of one script execution.

> l[const.available-threads]
> `AVAILABLE_THREADS` is a positive non-zero number.
> It is the amount of compute threads available to the application.
> It may be thought of as the number of cores available on the node, but the exact value is a concern for the control plane.

> l[const.available-memory]
> `AVAILABLE_MEMORY` is a positive non-zero number.
> It is the amount of memory, in bytes, available to the application.
> It may be thought of as the total RAM on the node, but the exact value is a concern for the control plane.

> l[const.cpu-architecture]
> `CPU_ARCHITECTURE` is a non-empty string identifying the CPU architecture of the node.
>
> Common values include `x86_64`, `aarch64`, `arm`, and `riscv64`, but other values may appear as platforms evolve.
> Scripts should treat unknown values as "unsupported" rather than error out.

> l[const.host-has-ipv4]
> `HOST_HAS_IPV4` is a boolean.
> It is `true` when the node has working IPv4 egress, and `false` otherwise.
>
> "Working egress" means a default IPv4 unicast route is configured. Source address selection is not inspected: an RFC1918 host behind NAT is considered to have egress.

> l[const.host-has-ipv6]
> `HOST_HAS_IPV6` is a boolean.
> It is `true` when the node has working IPv6 egress to the internet, and `false` otherwise.
>
> "Working egress" means both a default IPv6 unicast route and at least one globally-routable IPv6 source address. ULAs and link-local addresses do not qualify.

> l[const.nat64-active]
> `NAT64_ACTIVE` is a boolean.
> It is `true` when the node itself is providing NAT64 translation for its workloads, and `false` otherwise.
>
> A `false` value does not imply that IPv4-only workloads cannot reach IPv4 destinations: an external NAT64+DNS64 infrastructure may still be in play. Scripts that need a definitive answer should combine `HOST_HAS_IPV4` and `NAT64_ACTIVE`.

> l[const.has-snapshots]
> `HAS_SNAPSHOTS` is a boolean.
> It is `true` when the node's volume storage supports copy-on-write snapshots, and `false` otherwise.

> l[const.node-name]
> `NODE_NAME` is a string identifying the node the application runs on.
>
> The format is not specified. It may be empty in contexts where the node identity is not meaningful, such as when validating scripts outside of a running node.

> l[const.timezone]
> `TIMEZONE` is a non-empty string identifying the host's IANA timezone, such as `Pacific/Auckland` or `Etc/UTC`.
>
> When the host's local timezone cannot be determined, the value is `UTC`.

## OnUpdate

`OnUpdate` defines strategies for when [Deployments](#l--deployment.type) update.

> l[const.on-update.rolling]
> The `OnUpdate.Rolling` strategy first starts at least one _new_ container, waits until it becomes ready, then stops the same amount of _old_ containers, and repeats until all containers in the Deployment have been rotated to new versions.

> l[const.on-update.replace]
> The `OnUpdate.Replace` strategy stops all _old_ containers, even if that violates the Deployment's [scale lower bound](#l--deployment.scale), and only then starts the _new_ versions.

## OnTerminate

`OnTerminate` defines strategies for when [Containers](#l--container.interface) within [Deployments](#l--deployment.type) terminate.

> l[const.on-terminate.recreate]
> The `OnTerminate.Recreate` strategy always recreates the container when it terminates.

This is currently the only value.

## OnExit

`OnExit` defines strategies for when commands within [Containers](#l--container.interface) exit.

> l[const.on-exit.restart]
> The `OnExit.Restart` strategy always restarts the container when its command exits.

> l[const.on-exit.terminate]
> The `OnExit.Terminate` strategy always terminates the container when its command exits.

> l[const.on-exit.restart-on-failure]
> The `OnExit.RestartOnFailure` strategy restarts the container when its command exits with a non-zero exit status, and terminates it otherwise.

## ResourceType

> l[const.resource-type.enum]
> `ResourceType` is an opaque enum type, and in the script scope, is a constant object map of names to opaque values of type `ResourceType`.
>
> - `Parameter`
> - `Service`
> - `HttpService`
> - `Ingress`
> - `Deployment`
> - `Job`
> - `Volume`
> - `ExternalVolume`
> - `Action`

# App global

> l[app.var]
> `app` is a global variable available to every BSL script at the top level (and below).

> l[app.type]
> The `app` global variable is of type `App`.

> l[app.constructor]
> The `App` type is not constructible within a BSL.

> l[app.methods]
> All the methods of `app` are defined in this spec.
>
> Methods are either **resource methods**, which define resources, or **query methods**, which query the `App` state.

> l[app.resources]
> An `app` holds [Resources](#l--bsl.resource) defined against it.

> l[app.resources.static]
> Resources that are defined at the top level (outside of all actions) are said to be **static**.

> l[app.resources.dynamic]
> Resources that are not static are said to be **dynamic**.
> Dynamic resources are created inside action closures and exist only for the duration of the operation.
> They are cleaned up automatically when the action ends.

> l[app.resources.context.named]
> In the **static context** (top-level script), `app.resource(name)` creates and registers a named resource.
> In the **action context** (inside an action closure), `app.resource(name)` returns a **reference** to an existing static resource.
> If no static resource with that name exists, it is a script error.

> l[app.resources.context.immutable]
> Static resources are immutable in the action context.
> Calling a builder method on a static resource inside an action closure is a script error, regardless of how the resource handle was obtained — whether re-fetched via `app.resource(name)` inside the closure or captured from an outer-scope `let` binding.
> This applies to every builder method on every static resource type (e.g. `Volume.write`, `Service.exported`, `Deployment.healthcheck`, `Job.deadline`, `Ingress.tls`).
> Anonymous resources created inside the action context are mutable for the lifetime of the operation.

> l[app.resources.context.anonymous]
> Anonymous resources (those created without a name argument) may only be created in the action context.
> Attempting to create an anonymous resource at the top level is a script error.
> `Ingress` and `ExternalVolume` have no anonymous form in any context.

> l[app.resources.names]
> Most resources are defined with a name.
> If two methods use the _same name_, the methods return (a different handle to) the _same resource_.
>
> ```rhai
> let a = app.volume("data");
> let b = app.volume("data");
> // these are the same volume
> ```
>
> Names are also used to select resources using the Collection methods.
> Named resources created in the action context are references to existing static resources, not new definitions; see `l[app.resources.context.named]`.

> l[app.description]
> The `app.description(text: string)` method attaches a free-form human-readable description to the application as a whole. The description is surfaced to operators in the management interfaces (CLI, web UI, OI) alongside the app name.
>
> The description is purely informational and has no behavioural effect. Calling `description()` more than once on the app replaces the previous value. The description is `None` by default.
>
> The method returns `app` so calls may be chained.

# Parameter

> l[param.type]
> A Parameter is a value provided by the Seedling control plane to a BSL script, at a particular name.
>
> Parameters are defined using the `app.param(name: string)` method, which returns a `Param`.

> l[param.is-set]
> `param.is_set()` returns `true` if the operator has stored a value for this parameter, `false` otherwise.

> l[param.value]
> `param.value()` returns the parameter's current string value.
> If no value has been stored but a [default](#l--param.schema.default-value) has been declared on the `Param`, the default is returned.
> If no value has been stored and no default is declared, it throws.
>
> `param.is_set()` is not affected by the default: it remains `false` while the default is the effective value, so scripts can distinguish "operator-provided" from "defaulted".

> l[param.on-change]
> `param.on_change(fn: closure)` registers a handler that is called when the parameter's value changes.
>
> The `fn` closure may take up to two arguments: the [Runtime Instance](#l--rt.var) (typically named `rt`) and the previous `App` instance (typically named `old`).

> l[param.on-change.old]
> The `old` argument is an `App` value that reflects the state at the previous [generation](#r--generation.definition): the script evaluated with the parameter values as they were before the change.
>
> `old.param(name).is_set()` and `old.param(name).value()` return results consistent with the prior parameter state.
> Static resources defined in the script, and actions, are accessible via `old` and reflect their prior definitions. This is useful for resources whose shape depends on parameter values.
>
> Calling resource-definition methods on `old` (that would create or mutate resources) has no effect on the current app; `old` is a read-only view of the previous generation.

> l[param.on-change.transitions]
> The handler is invoked as a [lifecycle operation](#r--operation.lifecycle) when the effective value of the parameter transitions between states. For this purpose the value of a parameter is always `Option<Value>`: either unset (`None`) or set to some string (`Some(s)`).
>
> The handler is invoked on any of the following transitions:
>
> - `None` → `Some(s)`: the parameter was unset and has now been set.
> - `Some(s₁)` → `Some(s₂)` where `s₁ ≠ s₂`: the value has changed.
> - `Some(s)` → `None`: the parameter has been unset.
>
> Within the closure, `old.param(name).is_set()` reflects the prior half of the transition, and `app.param(name).is_set()` reflects the new half.

> l[param.on-change.not-on-install]
> The handler is not invoked during the initial install of an application, because there is no prior generation to compare against.

> l[param.on-change.constraints]
> `on_change` may only be called at the top level of the script (statically). Calling it from within an action closure must throw.
> Calling `on_change` more than once on the same parameter must throw.

> l[param.schema]
> A `Param` returned by `app.param()` may have schema metadata attached to it using builder methods. These methods return the same `Param` so calls can be chained.
>
> Schema metadata describes how the parameter should be presented and validated in management interfaces. Defaults: `kind` is `"text"`, `required` is `false`, `default_value` is absent, `description` is absent, `secret` is `false` (unless implied by `kind`).

> l[param.schema.kind]
> `param.kind(kind: string)` sets the kind of the parameter. Valid values are the same as for [Install Action params](#l--action.install.requirements): `"text"`, `"multiline"`, `"email"`, `"password"`, `"weak-password"`. Providing an unknown kind must throw.

> l[param.schema.required]
> `param.required(required: bool)` sets whether the parameter is required.

> l[param.schema.default-value]
> `param.default_value(value: string)` sets the default value for the parameter.

> l[param.schema.description]
> `param.description(description: string)` sets the human-readable description for the parameter.

> l[param.schema.secret]
> `param.secret(secret: bool)` marks the parameter as sensitive. When `true`, the parameter's value must be stored with confidentiality at rest, must never be returned to API clients or emitted in events, and must not appear in logs.

> l[param.schema.secret-from-kind]
> Parameters with `kind` `"password"` or `"weak-password"` are implicitly secret: their `secret` flag is treated as `true` unless explicitly overridden with `param.secret(false)`.
> The `secret` builder may be called after `kind` to override this implication in either direction.

# Service

> l[service.type]
> A Service is a network endpoint that operators and other resources can access.
> 
> Services are defined using the `app.service(name: string)` method, which returns a [builder](#l--bsl.builder).

> l[service.port]
> A Service Port is a particular port on a Service.
>
> Service Ports are defined using the `service.port(port: number)` instance method, which returns a `ServicePort`.
>
> The port number is "endpoint-side": connecting to the Service at that port number reaches the configured Service Port, but that might be _mapped_ to a different port number "pod-side".

> l[service.routing]
> Services accept TCP and UDP traffic as long as they have places to route it to.
> If there is no target for some traffic, it is dropped or rejected (implementation-defined).
> If there are multiple targets for the same traffic, it is distributed round-robin.

## HTTP Service

> l[service.http]
> A Service can be _specialised_ into an HTTP Service, using the `service.http(port?: number)` instance method, which returns an `HttpService`.
>
> The `port` argument is optional and defaults to `80`.
>
> An `HttpService` is a per-call view of the underlying Service rather than a distinct resource. As an ergonomic affordance, `httpService.ingress(hostname, port)` is accepted and is equivalent to calling `ingress()` on the wrapped Service: the resulting Ingress is bound to the Service. Calling `ingress()` on an `HttpService` whose Service is external must throw.

> l[service.http.route]
> An HTTP Service Route serves a URL prefix.
>
> HTTP Service Routes are defined using the `http.route(prefix: string)` instance method, which returns an `HttpServiceRoute`. The `prefix` argument must be a non-empty string starting with `/`.
>
> The URL prefix is _not_ stripped for the pod: `GET /api/books` routed through a `route("/api")` will appear as `GET /api/books` to the container.
>
> Prefix-matching is done by length: for any given URL, the longest matching prefix is selected. If more complicated logic is required, an application should embed an HTTP "reverse proxy" container of its choice.

> l[service.exported]
> `service.exported(options?: #{ description?: string })` is a builder method which marks the service as exported. Exported services are advertised to the control plane and operators.
>
> Only named static services can be exported. Calling `exported()` on an anonymous service must throw.

## External Service

> l[service.external]
> An External Service is a Service provided by the Seedling control plane to a BSL script, at a particular name.
>
> External Services are defined using the `app.external_service(name: string)` method, which returns an `ExternalService`.
>
> External Services can't be modified or configured further, only [mounted](#l--container.mount-service) or [bound](#l--pod.bind) like any other Service. The concrete endpoint the slot resolves to is supplied by the operator via a mapping; see the runtime spec [service.external.mapping.events](runtime.md#r--service.external.mapping.events) for the mapping lifecycle.

# Ingress

> l[ingress.type]
> An Ingress is an externally-accessible endpoint to the application.
>
> Ingresses are created from [Services](#l--service.type) using the `service.ingress(hostname: string, port: number)` instance method, which returns a [builder](#l--bsl.builder).
>
> Traffic from an Ingress is matched by the hostname and port, and sent to the associated Service at the same port.
>
> There can be multiple ingresses for a Service.

> l[ingress.hostname]
> The `hostname` argument must be a valid fully-qualified domain name: one or more labels separated by `.`, where each label is 1–63 ASCII alphanumeric or hyphen characters, must not start or end with a hyphen, and the overall length (including dots) must not exceed 253 characters.
>
> Wildcard labels (e.g. `*.example.com`) are not permitted. If wildcard matching is needed in the future it will be designed as a separate feature.
>
> If the hostname is not valid, the method must throw.

> l[ingress.conflicts]
> If more than one ingress matches the same (hostname, port) tuple...
> - ...within the same application: the latter definition in execution order will throw, and not be registered against the ingress. This can be caught (with `try..catch`) and handled.
> - ...between two or more applications: this is a control plane concern.

> l[ingress.certificates]
> This rule applies to all ingress spec rules that deal with certificates.
>
> Certificates will be automatically obtained whenever possible for the ingress's hostnames. The application will not have access to the key material.

> l[ingress.service]
> The `ingress.service()` instance method returns the Service that the ingress was created from.

> l[ingress.termination]
> The `ingress.tls(terminate: Terminate, output: Output)` builder method declares both what is terminated at the edge and what protocol the ingress hands to the bound Service.
>
> Exactly four `(terminate, output)` pairings are accepted; any other pairing must throw:
>
> | `terminate`        | `output`        | Behaviour |
> | ------------------ | --------------- | --------- |
> | `Terminate.Tls`    | `Output.Tcp`    | Terminate TLS for incoming TCP, re-emit as plaintext TCP. The ingress does not interpret the application protocol; non-TLS TCP traffic is rejected. |
> | `Terminate.Dtls`   | `Output.Udp`    | Terminate DTLS for incoming UDP, re-emit as plaintext UDP. Non-DTLS UDP traffic is rejected. |
> | `Terminate.Https`  | `Output.Http1`  | Terminate HTTPS (HTTP/1.1 and HTTP/2 over TCP, HTTP/3 over UDP), re-emit as plaintext HTTP/1.1. Non-HTTP traffic is rejected. |
> | `Terminate.Https`  | `Output.Http2`  | Terminate HTTPS, re-emit as plaintext HTTP/2 (`h2c`). Non-HTTP traffic is rejected. |
>
> Calling `tls()` without arguments, or with any other pairing, must throw with a clear message naming the supplied pair and the valid set.
>
> An ingress with no termination call is left as plain TCP passthrough.

> l[const.terminate.tls]
> The `Terminate.Tls` constant tags an ingress termination mode that strips TLS from incoming TCP traffic and exposes the inner bytes as plaintext TCP, without interpreting any application protocol.

> l[const.terminate.dtls]
> The `Terminate.Dtls` constant tags an ingress termination mode that strips DTLS from incoming UDP traffic and exposes the inner datagrams as plaintext UDP.

> l[const.terminate.https]
> The `Terminate.Https` constant tags an ingress termination mode that terminates HTTPS (HTTP/1.1 and HTTP/2 for TCP, HTTP/3 for UDP) and re-emits HTTP traffic to the bound Service.

> l[const.output.tcp]
> The `Output.Tcp` constant tags an ingress output protocol of plaintext TCP.

> l[const.output.udp]
> The `Output.Udp` constant tags an ingress output protocol of plaintext UDP datagrams.

> l[const.output.http1]
> The `Output.Http1` constant tags an ingress output protocol of plaintext HTTP/1.1.

> l[const.output.http2]
> The `Output.Http2` constant tags an ingress output protocol of plaintext HTTP/2 (`h2c`).

> l[ingress.redirect]
> The `ingress.redirect(port?: number, code?: number)` builder method emits an HTTP redirect on the `port` given if and when the ingress has obtained a TLS certificate.
>
> The `port` defaults to 80.
> The `code` defaults to 307 ([Temporary Redirect](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Status/307)).
>
> Calling this on an ingress whose termination is not `Terminate.Https` throws.

# Deployment

> l[deployment.type]
> A Deployment is a long-lived instance of a container workload. It describes how to run a single container image and associated configuration, and will manage updates to the underlying resource (the running container) from its declarative configuration.
>
> Deployments are defined using the `app.deployment(name: string)` method, which returns a [builder](#l--bsl.builder).

> l[deployment.pod]
> Deployment implements the [Pod](#l--pod.interface) interface.

> l[deployment.scale]
> The `deployment.scale(fixed: number)` or `deployment.scale(scalable: range)` builder method defines the scale ("replicas" in Kubernetes terms) of a Deployment.
>
> A fixed-scale Deployment will try to always keep that amount of container copies alive as long as the Deployment is running, not more or less. It is equivalent to `scale(fixed..fixed)`.
> The `fixed` number must be a positive non-zero integer.
>
> A scalable Deployment is defined from a lower and upper bound (represented as a range of positive integers). The Deployment will try to keep at least the lower bound and at most the upper bound of containers running, and operators or the Seedling control plane may modify the scale of the Deployment within the defined range. The lower bound may be zero. The upper bound must be non-zero.
>
> If a Deployment has a lower bound scale of zero it will be scheduled with zero containers initially.

> l[deployment.scale.max-lower-bound]
> The lower bound of a scale definition must not exceed 10. If a fixed scale or the lower bound of a range exceeds 10, the method must throw.

> l[deployment.on-update]
> The `deployment.on_update(strategy: OnUpdate)` builder method defines the strategy used when an update is applied to a Deployment.
> The default is [`OnUpdate.Rolling`](#l--const.on-update.rolling).

> l[deployment.on-terminate]
> The `deployment.on_terminate(strategy: OnTerminate)` builder method defines the strategy used when the controlled container terminates within a Deployment.
> The default is [`OnTerminate.Recreate`](#l--const.on-terminate.recreate).

> l[deployment.healthcheck]
> The `deployment.healthcheck(config: map)` builder method declares a periodic health check for the Deployment's container.
> The `config` map must contain a `kind` key identifying the check variety, plus zero or more common timing fields and zero or more kind-specific fields.
>
> Healthchecks are only valid on Deployments. The method is not registered on [Jobs](#l--job.type), so calling it on a Job is a BSL evaluation error.
>
> Common fields (all optional):
>
> - `interval`: seconds between successive checks. Default 30.
> - `timeout`: seconds a single check may run before being considered a failure. Default 30.
> - `retries`: consecutive failures required to transition from healthy to unhealthy. Default 3.
> - `start_period`: seconds of grace after the container starts during which failures do not count against `retries`. Default 0.
> - `on_failure`: response to sustained unhealth, one of `"replace"` or `"monitor"`. Default `"replace"`.
>
> All timing fields must be non-negative. `retries` must be a positive integer.
>
> If `healthcheck` is not called, the Deployment has no declared health check. A running container with no declared check is treated as healthy.

> l[deployment.healthcheck.kind]
> The `kind` field is required.
>
> In the current spec, the only accepted value is `"command"`. The values `"http"`, `"tcp"`, and `"grpc"` are reserved for future use and must be rejected by the current implementation with an error identifying the kind as unsupported.
>
> Any other value must cause the method to throw.

> l[deployment.healthcheck.command]
> When `kind` is `"command"`, the `config.cmd` field is required and defines the probe command run inside the container. The command is considered passing when it exits with code zero, and failing otherwise.
>
> `cmd` may be:
>
> - A string: run through a shell (equivalent to `["CMD-SHELL", cmd]`).
> - A string array: run directly, the first element being the executable.
>
> An empty `cmd` must cause the method to throw.

> l[deployment.healthcheck.timings]
> The timing fields control how often the probe runs and how quickly a failing probe transitions the container from healthy to unhealthy:
>
> - `interval`: seconds between successive checks. Default 30.
> - `timeout`: seconds a single check may run before being considered a failure. Default 30.
> - `retries`: consecutive failures required to transition from healthy to unhealthy. Default 3.
> - `start_period`: seconds of grace after the container starts during which failures do not count against `retries`. Default 0.
>
> All timing values must be non-negative; `retries` must be a positive integer. The grace window before a starting container is treated as unhealthy is `start_period + retries × interval`.

> l[deployment.healthcheck.on-failure]
> The `on_failure` response determines how the runtime reacts to a Deployment instance that has been unhealthy long enough to exceed the grace window:
>
> - `"replace"` (default): the runtime spawns a replacement instance alongside the unhealthy one and lets the unhealthy instance keep serving traffic until the replacement is healthy. When the replacement is healthy, traffic shifts to it and the unhealthy instance is retired. If the replacement also fails to become healthy, the runtime stops the cycle, leaves the original running (degraded), and files a hard fault per [fault.healthcheck-replace-failed](runtime.md#r--fault.healthcheck-replace-failed). See [autonomous.healthcheck-replace](runtime.md#r--autonomous.healthcheck-replace).
> - `"monitor"`: no automatic replacement. The container is observed and routing decisions account for its health (see [lifecycle.service](runtime.md#r--lifecycle.service)), but the runtime does not spawn replacements. Recovery is operator-driven.
>
> `on_failure` does not affect whether the container is considered Ready — an unhealthy container is not Ready regardless of the policy.

# Job

> l[job.type]
> A Job is a short-lived, one-off instance of a container workload.
>
> Jobs are defined using the `app.job(name: string)` method, which returns a [builder](#l--bsl.builder).
>
> A Job defined in the **static scope** (top level of the BSL script, outside any action or shell closure) is part of the application's steady state and will be included when `rt.start(app)` is called. A static Job has a single, fixed all-zero instance ID; only one instance of it may exist at a time.
>
> A Job defined in a **dynamic scope** (inside an action or shell closure) is not part of steady state. Each invocation of the enclosing action creates a new instance with an identity derived from that action invocation, so that two concurrent invocations of the same action can each run the Job without collision. See the runtime spec [Job instance identity](#r--identity.job) for the derivation.
>
> A Job passed to a shell `attach()` call receives a fresh randomly-generated instance ID, allowing multiple concurrent shell sessions against the same Job definition.

> l[job.pod]
> Job implements the [Pod](#l--pod.interface) interface.

> l[job.deadline]
> The `job.deadline(seconds: number)` builder method specifies how long the job can run for until it is terminated.
>
> This starts counting from when the Job enters the `Running` state.
>
> If there is no deadline, the job runs indefinitely.

# Container

> l[container.interface]
> Container is an interface (you can't obtain a `Container`-typed value) for the common builder methods, instance methods, and semantics of container workload definitions.

> l[container.image]
> The `container.image(uri: string)` builder method sets the container image reference.
> Image references must be fully qualified: `registry/path:tag` or `registry/path@algorithm:hex`.
> The registry component is the hostname (with optional port) before the first `/` and must contain at least one `.` or a `:` to distinguish it from a path component.
> A tag or digest must be present; bare `registry/path` references without a version specifier are rejected.
>
> A container without an `image` set may be inoperable.

> l[container.image.registry-allowlist]
> After BSL evaluation, every image reference is checked against the operator-configured registry allowlist.
> If an image's registry is not in the allowlist, a fault of kind `disallowed_registry` is filed for the app.
> The fault is cleared when the app is re-evaluated and all image registries pass the check.
> The default allowlist contains `docker.io` and `ghcr.io`.

> l[container.command]
> The `container.command(name: string)` or `container.command(entrypoint: string[])` builder method overrides the container's entrypoint (the executable to run).
> The `command(name: string)` form is equivalent to `command([name])`.
>
> This follows the [Kubernetes `command:` field](https://kubernetes.io/docs/tasks/inject-data-application/define-command-argument-container/) convention, which maps to `--entrypoint` in Docker/Podman — not to the positional arguments after the image name.

> l[container.arg]
> The `container.arg(var: string)` or `container.arg(vars: string[])` builder method appends arguments passed to the container's entrypoint (overriding the image's default `CMD`).
> The `arg(var: string)` form is equivalent to `arg([var])`.
>
> This follows the [Kubernetes `args:` field](https://kubernetes.io/docs/tasks/inject-data-application/define-command-argument-container/) convention, which maps to the positional arguments after the image name in Docker/Podman.

> l[container.env]
> The `container.env(name: string, value: string)` or `container.env(#{ name: string, value: string }[])` builder method inserts variables into the environment of the container.
> The `env(name: string, value: string)` form is equivalent to `env(#{ name: name, value: value })`.
>
> Environment variables set with the same name as previous variables override the earlier ones. That is, `.env("MANUKA", "honey").env("MANUKA", "branch")` is equivalent to `.env("MANUKA", "branch")`.

> l[container.env.validation]
> Environment variable names must be non-empty, consist only of ASCII letters, digits, and underscores, and must not start with a digit.
>
> The following environment variable names are forbidden and must cause the method to throw:
> `PATH`, `LD_PRELOAD`, `LD_LIBRARY_PATH`, `LD_AUDIT`, `LD_DEBUG`, `LD_PROFILE`.
>
> Environment variable values must not contain null bytes. If a value contains a null byte, the method must throw.

> l[container.mount-volume]
> The `container.mount(mountpoint: string, volume: Volume)` builder method binds a [volume](#l--volume.type) into the filesystem of the container at a given `mountpoint`.
> An [External Volume](#l--volume.external) can also be used.
>
> Mounts bound to a mountpoint identical to a previous mount override the earlier one.
>
> The `mountpoint` argument must be a unix-style path.

> l[container.mount-volume.validation]
> The `mountpoint` must be an absolute path (starting with `/`).
>
> The following mountpoints are forbidden and must cause the method to throw: `/`, `/proc`, `/sys`, `/dev`, `/etc`, `/bin`, `/sbin`, `/lib`, `/lib64`, `/usr`, `/boot`, `/run`.
> A mountpoint whose canonicalised form (after resolving `.` and `..` segments and collapsing repeated `/` separators, without touching the filesystem) equals any forbidden path is also rejected.
>
> The mountpoint must not contain null bytes. If it does, the method must throw.

> l[container.on-exit]
> The `container.on_exit(strategy: OnExit)` builder method defines the strategy used when the command exits.
> The default is [`OnExit.Restart`](#l--const.on-exit.restart) for Deployments and [`OnExit.Terminate`](#l--const.on-exit.terminate) for Jobs.

> l[container.memory]
> The `container.memory(limit: string)` builder method sets the memory limit for the container.
> The `limit` must be a positive integer followed by a unit suffix: `k` (kibibytes), `m` (mebibytes), or `g` (gibibytes). The suffix is case-insensitive.
> If no memory limit is set, the container has no memory constraint.

> l[container.cpus]
> The `container.cpus(limit: float)` builder method sets the CPU limit for the container.
> The `limit` must be a positive number. Fractional values are permitted (e.g. `0.5` for half a CPU core).
> If no CPU limit is set, the container has no CPU constraint.

> l[container.cap-add]
> The `container.cap_add(capability: string)` builder method adds a Linux capability to the container.
> Containers run with all capabilities dropped by default; this method selectively re-grants individual capabilities.
> The `capability` must be a valid Linux capability name (e.g. `"NET_RAW"`, `"NET_BIND_SERVICE"`). The name is case-insensitive and is normalised to uppercase.

> l[container.writable-rootfs]
> The `container.writable_rootfs()` builder method opts the container out of the default read-only root filesystem.
> By default, the container's root filesystem is mounted read-only; a writable tmpfs is provided at `/tmp`.

> l[container.pids-limit]
> The `container.pids_limit(limit: int)` builder method sets the maximum number of simultaneous PIDs permitted in the container.
> The `limit` must be a positive integer.
> The default PID limit is 256.

> l[container.workdir]
> The `container.workdir(path: string)` builder method sets the working directory of the container process.
> The `path` must be an absolute path.
> If not set, the working directory is determined by the container image.

> l[container.stop-signal]
> The `container.stop_signal(name: string)` builder method declares the POSIX signal sent to the container's main process when the runtime stops the container.
> Accepted forms are the canonical `"SIGFOO"` (e.g. `"SIGINT"`, `"SIGQUIT"`, `"SIGTERM"`) and the bare `"FOO"` shorthand; unknown signal names must be rejected at script-evaluation time.
> When unset, the default signal applied by the runtime is `SIGTERM`.
> Use cases: stateful workloads whose semantics require a specific signal — for example, PostgreSQL maps `SIGINT` to "fast shutdown" and `SIGTERM` to "smart shutdown" (which waits for clients to disconnect and may exceed the stop deadline).

> l[container.stop-timeout]
> The `container.stop_timeout(seconds: int)` builder method sets how long the runtime waits between sending the stop signal and sending `SIGKILL`.
> `seconds` must be a positive integer.
> When unset, the runtime applies its default (currently 90 seconds, matching systemd's `TimeoutStopSec` default).

# Pod

> l[pod.interface]
> Pod is an interface (you can't obtain a `Pod`-typed value) for the common builder methods, instance methods, and semantics of pod definitions.
>
> - Container has an image, a filesystem namespace, and runs the command;
> - Pod has a network namespace, and holds the Container.
>
> Not all things that implement Container implement Pod, but all things that implement Pod also implement Container. (Non-normative: this is not true of the current version of the spec, but the distinction is here for future expansions.)

> l[pod.mount-serviceport]
> The `pod.mount(svc: ServicePort)` builder method binds a ServicePort into the network of the pod. This makes the Service reachable at the declared port number from within the container, without the application needing to perform service discovery. The address at which the service is reachable is implementation-defined.

> l[pod.http]
> The `pod.http(port: number, svc: HttpServiceRoute)` or `pod.http(port: number, svc: HttpService)` builder method attaches a `port` of the pod to an [HTTP Service Route](#l--service.http.route).
> The `http(port, svc: HttpService)` form is equivalent to `http(port, svc.route("/"))`.
>
> HTTP traffic to the HTTP Service Route will be routed to (and back from) the port on the pod.

> l[pod.tcp]
> The `pod.tcp(port: number, svc: ServicePort)` or `pod.tcp(port: number, svc: Service)` builder method attaches a `port` of the pod to a [Service Port](#l--service.port).
> The `tcp(port, svc: Service)` form is equivalent to `tcp(port, svc.port(port))`.
>
> TCP traffic to the Service Port will be routed to (and back from) the port on the pod.

> l[pod.udp]
> The `pod.udp(port: number, svc: ServicePort)` or `pod.udp(port: number, svc: Service)` builder method attaches a `port` of the pod to a [Service Port](#l--service.port).
> The `udp(port, svc: Service)` form is equivalent to `udp(port, svc.port(port))`.
>
> UDP traffic to the Service Port will be routed to (and back from) the port on the pod.

# Volume

> l[volume.type]
> A Volume is a directory containing data.
>
> Volumes are defined using the `app.volume(name?: string)` method, which returns a [builder](#l--bsl.builder). The `name` argument is optional: if it's not provided, the volume is anonymous.
>
> Volumes can be [mounted](#l--container.mount-volume) to a container's filesystem.

> l[volume.readonly]
> `volume.readonly()` is a builder method which declares this volume to be read-only.
> A read-only volume cannot be written to.

> l[volume.tmpfs]
> `volume.tmpfs()` is a builder method which declares this volume to be backed by tmpfs (a RAM-based filesystem).
> The contents of a tmpfs volume do not survive a host reboot.

> l[volume.write]
> `volume.write(path: string, contents: string)` is an instance method which writes some data to the volume at `path`.
> Any existing content at `path` is discarded or shadowed.

> l[volume.write.validation]
> The `path` argument must be an absolute path (starting with `/`), must not contain null bytes, and must not escape the volume root after canonicalisation (resolving `.` and `..` segments without touching the filesystem). A path that resolves to `/` itself is also forbidden.
>
> If validation fails, the method must throw.
>
> At the runtime level, files written into volumes must be created with restrictive permissions (no more than 0640).

> l[volume.exported]
> `volume.exported(options?: #{ description?: string })` is a builder method which marks the volume as exported. Exported volumes are advertised to the control plane and operators.
>
> Only named static volumes can be exported. Calling `exported()` on an anonymous volume must throw.

## External Volume

> l[volume.external]
> An External Volume is a Volume provided by the Seedling control plane to a BSL script, at a particular name.
>
> External Volumes are defined using the `app.external_volume(name: string)` method, which returns an `ExternalVolume`.
>
> External Volumes can't be modified or configured further, only [mounted](#l--container.mount-volume).

> l[volume.external.dynamic]
> When `app.external_volume(name)` is called within an action closure, the runtime checks operation-scoped volume bindings first, then falls back to the static external volume mapping table. Operation-scoped bindings are injected by the runtime for specific internal operations and are not operator-configurable.
>
> If the name resolves to an operation-scoped binding, the returned `ExternalVolume` references the bound path for the duration of the operation. The binding is removed when the operation ends.
>
> If the name does not match any operation-scoped binding, the lookup falls back to the static external volume mapping table as for any other external volume reference.
>
> The names used for operation-scoped bindings are not fixed strings: they are generated per invocation by the runtime and delivered to the action closure in reserved params. See [operation.volume-param](runtime.md#r--operation.volume-param).

# Action

> l[action.type]
> An Action is a mechanism made available to operators (and in some cases, used autonomously by the Seedling control plane) to perform a structured task on an application.
>
> Actions are defined using the `app.on_action(name: string, fn: closure, options?: object)` method, which returns an `Action`.
>
> Action implements [Collection](#l--collection.interface), the Action is treated as an opaque Resource.
>
> The `fn` closure must take exactly two arguments: the [Runtime Instance](#l--rt.var) (typically named `rt`) and the [param map](#l--action.params) (typically named `param`).
>
> The `options` [object map](https://rhai.rs/book/language/object-maps.html)'s available properties are described below:

> l[action.option-description]
> An Action's `description` option is free-form text provided to operators. It may describe what the action does or is for.

> l[action.option-params]
> An Action may declare a `params` schema in its `options` object. The value is an object map of param key to definition object, with the same structure and validation kinds as [Install Action params](#l--action.install.requirements).
>
> Params declared in the schema are validated against their kind and required/default rules before the action is scheduled. Additional params not mentioned in the schema are permitted and passed through as-is.
>
> If a `kind` value is provided but does not match any of the defined kinds, `on_action()` must throw.

> l[action.params]
> All action closures receive exactly two arguments: the [Runtime Instance](#l--rt.var) (`rt`) and a `Param` object map (`param`).
>
> The `param` is an arbitrary key-value map provided by the invoker. When no params are provided, `param` is an empty map (`#{}`).
>
> Param keys ending in `_volume` or `_filename` are reserved for internal use by the Seedling runtime. The runtime must reject operator-provided params whose keys end in either suffix. See [operation.volume-param](runtime.md#r--operation.volume-param) for how the runtime uses these suffixes to hand operation-scoped volumes to action closures.

## Start Action

> l[action.start]
> The specialised Start Action is used to define how the application is started. It is used autonomously by the Seedling control plane.
>
> It may be defined using the `app.on_action()` method with a `name` of `"start"`, or with the shorthand `app.on_start(fn: closure, options?: object)`, which returns an `Action`.
>
> The `fn` closure must take exactly two arguments: `rt` and `param`. When fired autonomously (boot, schedule), `param` is an empty map.
>
> If it is not defined, it defaults to the equivalent of:
> ```rhai
> rt.start(app);
> ```

> l[action.start.no-manual-invoke]
> The Start Action is a lifecycle-only action. It must not be manually invokable via the action invocation RPC. Attempting to invoke it must return `not_found`.

## Shell Action

> l[action.shell]
> A Shell Action is a specialised _kind_ of Action, which provide an interactive terminal session to an operator. Shells are never used autonomously by the Seedling control plane.
>
> Shells must be defined using the `app.on_shell(name: string, fn: closure, options?: object)` method, and cannot be defined using `on_action()`.
>
> Shells exist in a separate namespace as other actions: their names do not conflict.
>
> The `fn` closure must take exactly three arguments: the [Runtime Instance](#l--rt.var) (typically named `rt`), the [Shell Control](#l--action.shell.control) (typically named `shell`), and the param map (typically named `param`).
>
> A shell closure that returns without calling `shell.attach` or `shell.error` is invalid. The runtime must return an error to the client.

> l[action.shell.control]
> The Shell Control is the second argument of the Shell Action. It is a custom type with two methods:
>
> - `shell.attach(job: Job)`: bridges the operator's input/output to the Job. Blocks until the operator closes the session or the connection is interrupted. Must be called exactly once per shell invocation. Calling `attach` a second time must throw.
> - `shell.error(msg: string)`: sends an error message to the client and terminates the shell session. This call is terminal: it throws an exception to end the closure.

> l[action.shell.attach]
> The Shell Attacher is the second argument of the Shell Action. It is a host function which bridges the operator to the [Job](#l--job.type) provided as argument, attaching the operator's input/output to that of the Job.
>
> The Shell Attacher returns once the operator closes the shell, or if the connection is interrupted in some other way.
> <!-- TODO: consider a return value to indicate how it exited -->

## Scheduled Actions

> l[action.schedule]
> An Action returned by `on_action()` may be given one or more cron schedules via the `.on_schedule(expr: string)` builder method.
>
> `on_schedule` returns the Action for chaining. It may be called multiple times to attach multiple schedules to the same action.
>
> The `expr` is a 5-field cron expression (minute, hour, day-of-month, month, day-of-week) with 1-minute minimum resolution. The Jenkins `H` extension is supported: `H` is replaced with a stable hash-derived value within the field's range, computed from `(app_name, action_name)`. For example, `H 2 * * *` fires once daily at a stable minute during the 02:xx hour.
>
> An optional sixth field specifies the IANA timezone (e.g. `0 2 * * * Pacific/Auckland`). When omitted, the expression is interpreted in the system local timezone of the daemon.
>
> `on_schedule` must not be called on the Start Action (name `"start"`); doing so must throw. `on_schedule` is not available on Shell Actions.
>
> When a scheduled action fires, it is invoked as a normal lifecycle operation with an empty `param` map. Operators may also invoke a scheduled action manually via the action invocation RPC, in which case operator-provided params are passed through.

## Install Action

> l[action.install]
> The specialised Install Action is used to define how the application is first set up. It is used autonomously by the Seedling control plane.
>
> It must be defined using the `app.on_install(fn: closure, config?: object)` method, and cannot be defined using `on_action()`.
>
> The `fn` closure must take exactly two arguments: `rt` and `param`. Param values are delivered through `param`. The `config` object defines the validation schema; it does not change the closure signature.
>
> If it is not defined, it defaults to the equivalent of:
> ```rhai
> rt.action(app, "start");
> ```

> l[action.install.requirements]
> The Install Action can define special parameters which are only requested from the operator when installation is requested. The values of the params are only known to Seedling for the duration of the Install process, and are discarded afterwards.
>
> The param schema is declared under the `params` key of the `config` object passed to `on_install()`: `#{ params: #{ key: #{ ... }, ... } }`.
>
> The `param` argument of the `fn` _closure_ is an object map of param key to string value.
>
> Each param definition has these fields:
> - `kind` (optional): how to present/validate the field, defaults to `"text"`;
> - `required` (optional): boolean, defaults to `true`;
> - `description` (optional): free-form text, for the operator to understand what the value is or is for;
> - `default_value` (optional): string, the value to use if none is provided;
> - `secret` (optional): boolean; defaults to `false`, but `"password"` and `"weak-password"` kinds imply `true` unless explicitly set to `false`. When `true`, the value is treated as sensitive for the duration of the install operation.
>
> If `default_value` is set and `required` is `true`, the default value is pre-populated in the field input (but the field is still mandatory and cannot be submitted empty).

> l[action.install.requirements.kind-text]
> A param kind of `"text"` is a free-form text field.
> No validation is applied.

> l[action.install.requirements.kind-multiline]
> A param kind of `"multiline"` is a free-form text field that may contain multiple lines.
> No validation is applied.
> It is semantically equivalent to `"text"` but hints to presentation layers (such as a web UI) that a multi-line input control is appropriate.

> l[action.install.requirements.kind-email]
> A param kind of `"email"` is an email address field.
> Basic validation is applied, and hints may be provided for more outlandish values ([for example](https://en.wikipedia.org/wiki/Email_address#Valid_email_addresses), `" "@example.org` is a valid email address, but probably not what an user meant).

> l[action.install.requirements.kind-password]
> A param kind of `"password"` is a strong password field.
> Weak passwords must not be accepted (what makes a weak password is implementation-defined, but should be something like [zxcvbn](https://lowe.github.io/tryzxcvbn/)).
> The field should have a strong password generator available.

> l[action.install.requirements.kind-weak-password]
> A param kind of `"weak-password"` is a free-form password field.
> Password strength should be hinted, but must not restrict submission.
> The field should have a strong password generator available.

> l[action.install.requirements.kind-random]
> A param kind of `"random"` is a free-form text field that hints to presentation layers (such as a web UI) that a one-click generator should be offered for producing a random value, alongside manual text entry.
> When a generator is offered, its default output is 32 bytes encoded as lowercase hexadecimal (a 64-character string).
> No validation is applied; the kind is semantically equivalent to `"text"` for storage and submission.
> The kind does not imply [`secret`](#l--param.schema.secret) — scripts that store sensitive material in a `"random"` param must set `secret(true)` explicitly.

> l[action.install.requirements.kind-unknown]
> If a param `kind` is provided but does not match any of the defined kinds, `on_install()` must throw.

> l[action.params.volume]
> Action and shell param schemas accept an additional kind, `"volume"`, that the static schemas (`app.param`, install requirements) must reject. When an action or shell schema declares a param with `kind: "volume"`, the runtime asks the operator to pick a site volume at invocation time, validates that the chosen volume exists, and at dispatch time builds an operation-scoped binding under the reserved key `<param-name>_volume` per [operation.volume-param](runtime.md#r--operation.volume-param). The closure consumes it via `app.external_volume(param["<param-name>_volume"])`.
>
> The static schemas reject this kind because their bindings outlive any single operation; the equivalent for static configuration is a declared `external_volume` mapping wired up by the operator.

# Runtime Instance

The Runtime Instance is a handle to the Seedling runtime for an application.
It's how the script actually controls the containers in the "outside world".

This spec defines the semantics of the Runtime Instance as far as BSL is concerned; the exact implementation and control plane semantics are defined in other places, and must not be relied upon by BSL scripts.

> l[rt.var]
> `rt` is a variable available within actions (usually as the first argument of the closure).

> l[rt.type]
> The `rt` variable is of type `RuntimeInstance`.

> l[rt.constructor]
> The `rt` type is not constructible within a BSL.

> l[rt.methods]
> All the methods of `rt` are defined in this spec.

> l[rt.lifecyle]
> - _Pending_: the resource is active in the runtime state, but not yet scheduled.
> - _Scheduled_: the resource is set up "in the world" (on the node or cluster).
> - _Running_: the resource is running, but may not yet be ready.
> - _Ready_: the resource is ready to be used.
> - _Terminating_: termination has been initiated by the runtime.
> - _Terminated_: the resource has terminated.
> - _Unscheduled_: the resource has been cleaned up.
>
> It's the runtime's concern as to how the scheduling and other lifecycle actions and events work for each resource type.
>
> Note that a resource can transition directly from _Running_ or _Ready_ to _Terminated_, for example when it exits on its own.

## Workload control

> l[rt.start]
> The `rt.start(resources: Collection)` method schedules the resources in the Collection. It returns a [Started](#l--rt.started).

> l[rt.stop]
> The `rt.stop(resources: Collection, deadline?: number)` method unschedules the resources in the Collection and blocks until all terminate.
> `deadline` is interpreted the same as for [Started](#l--rt.started.state-methods).

> l[rt.query]
> The `rt.query(resources: Collection)` method returns a [Started](#l--rt.started) _without_ scheduling the resources.

> l[rt.restart]
> The `rt.restart(deployment: Deployment)` method triggers a restart of all running instances of the named Deployment, following its configured update strategy ([on_update](#l--deployment.on-update)).
> It does not change the deployment's definition or generation, and returns no value.
> Triggering semantics and durability are specified in [deployment.restart](../interface.md#i--deployment.restart) at the interface level.

> l[rt.warm-certs]
> The `rt.warm_certs(resources: Collection)` method selects all TLS-terminating [Ingresses](#l--ingress.type) from the given Collection and ensures their TLS certificates are provisioned and cached, without yet routing traffic to those ingresses.
> Non-ingress resources and ingresses without TLS termination in the collection are ignored.
>
> It returns a [Started](#l--rt.started). Calling `.ready()` on the returned `Started` blocks until certificates are valid for every selected ingress.
> If certificate provisioning cannot complete for one or more selected ingresses (for example, ACME issuance fails persistently), a fault is filed and the barrier throws when its deadline expires.
>
> If the selection contains no TLS-terminating ingresses, the returned `Started` is immediately satisfied.
>
> Calling `rt.warm_certs(app)` from within an [`on_change`](#l--param.on-change) handler warms exactly the ingresses that exist in the new generation. Ingresses present in the previous generation but not the new one are not warmed.

> l[rt.warm-images]
> The `rt.warm_images(resources: Collection)` method selects all [Deployment](#l--deployment.type) and [Job](#l--job.type) resources from the given Collection, extracts their container image references, and ensures those images are present in local container storage, without starting the containers.
> Non-container resources, and container resources that have no image reference declared, are ignored.
>
> Each distinct image reference in the selection is _pinned_ to the calling app: a pinned image is protected from autonomous image removal until a running container is observed using it (at which point the pin is cleared automatically) or an operator clears the pin explicitly. Pins are not automatically re-established when a workload that was using an image stops; a subsequent `rt.warm_images` call is required to re-pin.
>
> It returns a [Started](#l--rt.started). Calling `.ready()` on the returned `Started` blocks until every selected image is present locally.
> If an image pull fails persistently, a fault is filed and the barrier throws when its deadline expires.
>
> If the selection contains no images to warm, the returned `Started` is immediately satisfied.
>
> Anonymous container resources are supported: `rt.warm_images(app.job().image("registry/foo:1.2.3"))` warms the single image reference of a dynamic job without ever starting it, which is the idiomatic way to pre-load a future version of an image ahead of a deploy.

> l[rt.signal]
> The `rt.signal(target: Collection, signal: string)` method delivers the named POSIX signal to PID 1 of every running container instance in the selected Collection.
>
> The `signal` argument accepts the canonical `"SIGFOO"` form or its bare `"FOO"` shorthand; unknown signal names are a script-evaluation error.
>
> Selection rules:
>
> - Container instances that are not running are silently skipped (no error). This makes `rt.signal` safe to use against deployments where some replicas may be in transient states.
> - Non-container resources in the collection are ignored.
> - An empty selection is an error — typically a sign that the caller's collection expression resolved to nothing.
>
> The call is _at-most-once_ across replays: when the runtime restarts mid-operation and replays the action closure, a previously-delivered signal is not re-sent.
>
> Typical use cases: `SIGHUP` to reload configuration without restart (postgres, nginx), `SIGUSR1` to trigger log rotation, `SIGTERM`/`SIGINT` for cooperative shutdown when the deployment's `stop_signal` is not what the action wants for this one-off invocation.

> l[rt.write]
> The `rt.write(target: Volume | ExternalVolume, path: string, contents: string)` method writes a file into the given volume at action runtime, parallel to the static `Volume.write`.
>
> The target may be:
>
> - a named (static) `Volume` declared at the top level,
> - an anonymous `Volume` created earlier in the same action closure, or
> - an `ExternalVolume`, including those resolved from operation-scoped volume bindings (see `l[action.params.volume]`).
>
> The path must be absolute, must not contain `..` components, and must not resolve to the volume root; these are the same validation rules as `l[volume.write.validation]`.
>
> Unlike static `Volume.write`, `rt.write` does NOT reapply on container restart. It is a point-in-time write at action time. For tmpfs volumes that means the contents are erased on the next container start; this is allowed and is the user's responsibility to reason about.
>
> The call is _at-most-once_ across replays: when the runtime restarts mid-operation and replays the action closure, a previously-completed `rt.write` is not re-executed.
>
> Calling `rt.write` outside an action closure is a script error.

## Waiting on resource state

> l[rt.started.type]
> `Started` is an opaque type representing some resources that have been started.
>
> `Started` implements [Collection](#l--collection.interface), except that all the resources returned by Collection's methods return `Started`s corresponding to the resources, instead of the original resources.

> l[rt.started.state-methods]
> `Started` has a number of methods of the form `started.<state>(deadline?: number)` which block until all resources have entered the state `<state>` (one of `scheduled`, `running`, `ready`, `terminated`).
>
> The argument `deadline` must be a positive integer number of seconds; if it's zero or absent, the default deadline for that state is used (see [default deadlines](#l--rt.started.default-deadlines)).
>
> If the deadline is reached before the method returns, an exception is thrown.

> l[rt.started.default-deadlines]
> The default deadlines for `started.<state>()` barriers with no explicit `deadline` argument are:
>
> - `scheduled`, `running`, `ready`: short (on the order of tens of seconds), because these barriers guard correctness signals — a resource that has not reached these states in that window usually indicates a cluster-level problem rather than a slow workload.
> - `terminated`: long (on the order of hours), because `terminated` is routinely called on Jobs that run for extended periods.
>
> Every default deadline is a positive non-zero number of seconds. The exact values are set by the control plane and are not specified here.
>
> Callers with an unbounded wait requirement use [`started.terminated_eventually()`](#l--rt.started.terminated-eventually) or [`started.ready_eventually()`](#l--rt.started.ready-eventually) instead of passing a very large deadline.

> l[rt.started.terminated]
> The `started.terminated()` state method returns a [Termination](#l--rt.termination.type).

> l[rt.started.terminated-eventually]
> `started.terminated_eventually()` behaves like [`started.terminated()`](#l--rt.started.terminated), but has no deadline: it blocks until all resources reach `terminated` or the operation is [cancelled](#r--operation.cancel).
>
> Use this for jobs with genuinely unbounded duration (e.g. large backups, bulk restores) where any finite deadline is either too short to be safe or too long to be useful as a failure signal.

> l[rt.started.ready-eventually]
> `started.ready_eventually()` behaves like `started.ready()`, but has no deadline: it blocks until all resources reach `ready` or the operation is cancelled.
>
> Typical use: ingress cert provisioning via an external CA (e.g. Let's Encrypt) whose completion time is bounded only by the CA's rate limiting.

> l[rt.termination.type]
> `Termination` is an opaque type representing the termination state of a resource.

> l[rt.termination.ensure-success]
> The `termination.ensure_success()` method throws if the resource terminated without succeeding.
>
> For a container-backed resource (Deployment, Job), "succeeded" means the container's process exited with status 0. Any non-zero exit status — including termination by signal — is considered a failure. When the runtime observes the container's exit code directly, that observation is authoritative. When the exit code is unobservable (for example because the container was auto-removed before the runtime could inspect it), the runtime may fall back to secondary signals: a systemd-level unit failure counts as a failure; otherwise the run is treated as a success provided the instance reached a terminal lifecycle state.
>
> When a [Started](#l--rt.started.type) group holds multiple resources, all of them must have succeeded; otherwise `ensure_success()` throws.
>
> For other resource kinds with no natural exit code (Service, Ingress, Volume), termination itself is success.

<!-- TODO: more Termination methods -->
