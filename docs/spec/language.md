Beset Scripting Language, or BSL for short, is a DSL (Domain Specific Language).
It is used to define and manage an application running on a Beset node in an autonomous way and provide administrative controls to operators.

The terminology used in BSL closely resembles that used for [Kubernetes](https://kubernetes.io), but some of the semantics are different.

> l[bsl.syntax]
> BSL is written in [Rhai](https://rhai.rs).

> l[bsl.script]
> A BSL script is one or more code listings ("files") which share a [scope](#l--bsl.scope), and come together to define a Beset Application.

> l[bsl.scope]
> The runtime must use a distinct [Rhai Scope](https://rhai.rs/book/engine/scope.html) for each BSL script.

> l[bsl.errors]
> Some methods throw exceptions under some circumstances.
> The `try..catch` Rhai construct may be used to handle those exceptions and recover.
> If an exception bubbles to the top of the script, execution is considered failed and will not proceed further.
> Responding to this is a control plane concern and not defined in this spec.

> l[bsl.placeholder]
> A placeholder value is a special value which is returned from a method or function when its real value is not yet known.
> Placeholder values must not be used for real effects.
> Some placeholders are special opaque instances of a type, others are predefined values.
>
> Predefined values for non-opaque types:
> - string placeholder: `"<placeholder>"`

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
> The regular expression `^[a-zA-Z][a-Z0-9-]{1,60}[a-zA-Z0-9]$` may be used to validate a name.

> l[bsl.port]
> Various methods and resources use port numbers.
>
> Unless otherwise specified, a port number must be a non-zero positive integer below 65535.
> If an invalid port is provided, the method must throw.

> l[bsl.resource]
> The term Resource is using a similar definition [as Kubernetes](https://kubernetes.io/docs/reference/using-api/api-concepts/#standard-api-terminology):
> - A _resource type_ is the name used in the spec (Service, Deployment, Job)
> - A list of instances of a resource type is known as a _collection_
> - A single instance of a resource type is called a _resource_

> l[bsl.collection]
> A `Collection` is an abstract trait of things that can be one or more Resources.
> Workload control methods often operate on or with Collections.
> Collections can hold different resource types, and can hold Collections.
> Order within a collection is not defined.
>
> All Resources are themselves a Collection of the resource itself and all resources that are contained (not references) in it.
>
> An array of Collections is a Collection of the contents.

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

> l[app.resources.names]
> Most resources are defined with a name.
> If two methods use the _same name_, the methods return (a different handle to) the _same resource_.
>
> ```rhai
> let a = app.volume("data");
> let b = app.volume("data");
> // these are the same volume
> ```

> l[app.collection]
> `App` implements [`Collection`](#l--bsl.collection) thus:
> - all `Deployments`

# Parameter

> l[param.type]
> A Parameter is a string provided by the Beset control plane to a BSL script, at a particular name.
>
> Parameters are defined using the `app.param(name: string)` method, which returns their value when known.
> When the value is not yet known, `app.param()` returns the [placeholder string](#l--bsl.placeholder).

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

> l[service.http.route]
> An HTTP Service Route serves a URL prefix.
>
> HTTP Service Routes are defined using the `http.route(prefix: string)` instance method, which returns an `HttpServiceRoute`. The `prefix` argument must be a non-empty string starting with `/`.
>
> The URL prefix is _not_ stripped for the pod: `GET /api/books` routed through a `route("/api")` will appear as `GET /api/books` to the container.
>
> Prefix-matching is done by length: for any given URL, the longest matching prefix is selected. If more complicated logic is required, an application should embed an HTTP "reverse proxy" container of its choice.

## External Service

> l[service.external]
> An External Service is a Service provided by the Beset control plane to a BSL script, at a particular name.
>
> External Services are defined using the `app.external_service(name: string)` method, which returns a `ExternalService`.
> When the volume is not yet available, `app.external_service()` returns a [placeholder](#l--bsl.placeholder).
>
> External Services can't be modified, only [mounted](#l--pod.mount-service).

> l[service.external.port]
> `extsvc.port(port: number)` returns a [ServicePort](#l--service.port) _if the port is defined_ by the control plane on the external service.
> If the port is not defined, this will throw.

# Ingress

> l[ingress.type]
> An Ingress is an externally-accessible endpoint to the application.
>
> Ingresses are created from [Services](#l--service.type) using the `service.ingress(hostname: string, port: number)` instance method, which returns a [builder](#l--bsl.builder).
>
> Traffic from an Ingress is matched by the hostname and port, and sent to the associated Service at the same port.
>
> There can be multiple ingresses for a Service.

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

> l[ingress.tls]
> The `ingress.tls()` builder method terminates TLS for the TCP traffic to this ingress.
>
> The Ingress only terminates TLS, it does not interact with the TCP traffic.
> If non-TLS TCP traffic is sent to the ingress, it is rejected.

> l[ingress.dtls]
> The `ingress.dtls()` builder method terminates DTLS for the UDP traffic to this ingress.
>
> The Ingress only terminates DTLS, it does not interact with the UDP traffic.
> If non-DTLS UDP traffic is sent to the ingress, it is rejected.

> l[ingress.quic]
> The `ingress.quic()` builder method terminates QUIC for the UDP traffic to this ingress.
>
> The Ingress only terminates QUIC, it does not interact with the application traffic.
> The traffic is re-emitted _as QUIC_ with another certificate which must be ignored.
> If non-QUIC UDP traffic is sent to the ingress, it is rejected.
>
> If you want HTTP/3 termination, use [`ingress.http()`](#l--ingress.http).

> l[ingress.http]
> The `ingress.http()` builder method terminates HTTPS (HTTP/1.1 and HTTP/2 for TCP, HTTP/3 for UDP) traffic for this ingress.
>
> The Ingress only terminates HTTPS, it does not interact with the application traffic.
> The traffic is re-emitted as plaintext HTTP/1.1.
> If non-HTTP traffic is sent to the ingress, it is rejected.

> l[ingress.http2]
> The `ingress.http2()` builder method terminates HTTPS (HTTP/1.1 and HTTP/2 for TCP, HTTP/3 for UDP) traffic for this ingress.
>
> The Ingress only terminates HTTPS, it does not interact with the application traffic.
> The traffic is re-emitted as plaintext HTTP/2 (`h2c`).
> If non-HTTP traffic is sent to the ingress, it is rejected.

> l[ingress.redirect]
> The `ingress.redirect(port?: number, code?: number)` builder method emits an HTTP redirect on the `port` given if and when the ingress has obtained a TLS certificate for one of the HTTP terminations.
>
> The `port` defaults to 80.
> The `code` defaults to 307 ([Temporary Redirect](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Status/307)).
>
> Calling this on an ingress _not_ configured for HTTPS termination throws.

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
> A scalable Deployment is defined from a lower and upper bound (represented as a range of positive integers). The Deployment will try to keep at least the lower bound and at most the upper bound of containers running, and operators or the Beset control plane may modify the scale of the Deployment within the defined range. The lower bound may be zero. The upper bound must be non-zero.

> l[deployment.strategy]
> The `deployment.strategy(strategy: DeploymentStrategy)` builder method defines the strategy used when an update is applied to a Deployment.
> The default is `DeploymentStrategy.Rolling`.
> `DeploymentStrategy` is an opaque enum type, and in the script scope, is a constant object map of names to opaque values of type `DeploymentStrategy`.

> l[deployment.strategy.rolling]
> The `DeploymentStrategy.Rolling` strategy first starts at least one _new_ container, waits until it becomes ready, then stops the same amount of _old_ containers, and repeats until all containers in the Deployment have been rotated to new versions.

> l[deployment.strategy.replace]
> The `DeploymentStrategy.Replace` strategy stops all _old_ containers, even if that violates the Deployment's [scale lower bound](#l--deployment.scale), and only then starts the _new_ versions.

# Job

> l[job.type]
> A Job is a short-lived, one-off instance of a container workload.
>
> Jobs are defined using the `app.job(name: string)` method, which returns a [builder](#l--bsl.builder).

> l[job.container]
> Job implements the [Container](#l--container.interface) interface.

# Container

> l[container.interface]
> Container is an interface (you can't obtain a `Container`-typed value) for the common builder methods, instance methods, and semantics of container workload definitions.

> l[container.image]
> The `container.image(uri: string)` builder method sets the URI of the container image to be used for using this container.
> Image URIs are interpreted by the underlying container runtime provider, which may be [Podman](https://docs.podman.io/en/latest/markdown/podman-pull.1.html#source) or [Kubernetes](https://kubernetes.io/docs/concepts/containers/images/#image-names).
>
> A container without an `image` set may be inoperable.

> l[container.command]
> The `container.command(name: string)` or `container.command(entrypoint: string[])` builder method sets the container entrypoint and arguments passed to the image.
> The `command(name: string)` form is equivalent to `command([name])`.

> l[container.arg]
> The `container.arg(var: string)` or `container.arg(vars: string[])` builder method sets the container arguments passed to the image.
> The `arg(var: string)` form is equivalent to `arg([var])`.

> l[container.env]
> The `container.env(name: string, value: string)` or `container.env(#{ name: string, value: string }[])` builder method inserts variables into the environment of the container.
> The `env(name: string, value: string)` form is equivalent to `env(#{ name: name, value: value })`.
>
> Environment variables set with the same name as previous variables override the earlier ones. That is, `.env("MANUKA", "honey").env("MANUKA", "branch")` is equivalent to `.env("MANUKA", "branch")`.

> l[container.mount-volume]
> The `container.mount(mountpoint: string, volume: Volume)` builder method binds a [volume](#l--volume.type) into the filesystem of the container at a given `mountpoint`.
> An [External Volume](#l--volume.external) can also be used.
>
> Mounts bound to a mountpoint identical to a previous mount override the earlier one.
>
> The `mountpoint` argument must be a unix-style path.

# Pod

> l[pod.interface]
> Pod is an interface (you can't obtain a `Pod`-typed value) for the common builder methods, instance methods, and semantics of pod definitions.
>
> - Container has an image, a filesystem namespace, and runs the command;
> - Pod has a network namespace, and holds the Container.
>
> Not all things that implement Container implement Pod, but all things that implement Pod also implement Container.

> l[pod.mount-serviceport]
> The `pod.mount(svc: ServicePort)` builder method binds a ServicePort into the network of the pod. This makes the Service available at a particular port on `localhost` for the container.

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

> l[volume.write]
> `volume.write(path: string, contents: string)` is an instance method which writes some data to the volume at `path`.
> Any existing content at `path` is discarded or shadowed.

## External Volume

> l[volume.external]
> An External Volume is a Volume provided by the Beset control plane to a BSL script, at a particular name.
>
> External Volumes are defined using the `app.external_volume(name: string)` method, which returns an `ExternalVolume`.
> When the volume is not yet available, `app.external_volume()` returns a [placeholder](#l--bsl.placeholder).
>
> External Volumes can't be modified or configured further, only [mounted](#l--container.mount-volume).

# Action

> l[action.type]
> An Action is a mechanism made available to operators (and in some cases, used autonomously by the Beset control plane) to perform a structured task on an application.
>
> Actions are defined using the `app.on_action(name: string, fn: closure, options?: object)` method.
>
> The `fn` closure may take one argument, the [Runtime Instance](#l--rt.var), typically named `rt`. Specialised Actions may have access to more arguments.
>
> The `options` [object map](https://rhai.rs/book/language/object-maps.html)'s available properties are described below:

> l[action.option-description]
> An Action's `description` option is free-form text provided to operators. It may describe what the action does or is for.

## Start Action

> l[action.start]
> The specialised Start Action is used to define how the application is started. It is used autonomously by the Beset control plane.
>
> It may be defined using the `app.on_action()` method with a `name` of `"start"`, or with the shorthand `app.on_start(fn: closure, options?: object)`.
>
> If it is not defined, it defaults to the equivalent of:
> ```rhai
> rt.start(app);
> ```

## Upgrade Action

> l[action.upgrade]
> The specialised Upgrade Action is used to define how the application is upgraded. It is used autonomously by the Beset control plane.
>
> It may be defined using the `app.on_action()` method with a `name` of `"upgrade"`, or with the shorthand `app.on_upgrade(fn: closure, options?: object)`.
>
> Its `fn` closure may take up to two arguments: the [Runtime Instance](#l--rt.var) (typically named `rt`) and the `App` instance being replaced (typically named `old`).
>
> If it is not defined, it defaults to the equivalent of:
> ```rhai
> rt.stop(old);
> rt.action(app, "start");
> ```
> which is usually safe but incurs downtime.

## Crash Recovery Action

> l[action.crash-recovery]
> The specialised Crash Recovery Action is used to define how the application recovers from a Beset Node crash. It is used autonomously by the Beset control plane.
>
> A crash in this context is not an application failure, but a crash and restart of the entire node, usually caused by an unplanned power failure or some kind of system panic. If an application was in the middle of an upgrade, it may want to attempt to finish the upgrade, and/or run data integrity checks.
>
> It may be defined using the `app.on_action()` method with a `name` of `"crash_recovery"`, or with the shorthand `app.on_crash_recovery(fn: closure, options?: object)`.
>
> Its `fn` closure may take up to two arguments: the [Runtime Instance](#l--rt.var) (typically named `rt`) and the [Application History](#l--history.var) (typically named `history`).
>
> If it is not defined, it defaults to the equivalent of:
> ```rhai
> rt.action(app, "start");
> ```

## Shell Action

> l[action.shell]
> A Shell Action is a specialised _kind_ of Action, which provide an interactive terminal session to an operator. Shells are never used autonomously by the Beset control plane.
>
> Shells must be defined using the `app.on_shell(name: string, fn: closure, options?: object)` method, and cannot be defined using `on_action()`.
>
> Shells exist in a separate namespace as other actions: their names do not conflict.
>
> The `fn` closure must either:
> - take up to one argument (the [Runtime Instance](#l--rt.var), `rt`), and return a [Job](#l--job.type); or
> - take exactly two arguments, the [Runtime Instance](#l--rt.var) (typically named `rt`) and the [Shell Attacher](#l--action.shell.attach) (typically named `attach`), and return nothing.
>
> The first form is equivalent to using the second form and calling `attach` on the first form's return value. A Shell Action which does not call `attach` (implicitly via return or explicitly) is invalid, and may be unavailable for use.

> l[action.shell.attach]
> The Shell Attacher is the second argument of the Shell Action. It is a host function which bridges the operator to the [Job](#l--job.type) provided as argument, attaching the operator's input/output to that of the Job.
>
> The Shell Attacher returns once the operator closes the shell, or if the connection is interrupted in some other way.
> <!-- TODO: consider a return value to indicate how it exited -->

## Install Action

> l[action.install]
> The specialised Install Action is used to define how the application is first set up. It is used autonomously by the Beset control plane.
>
> It must be defined using the `app.on_install(fn: closure, requirements?: object)` method, and cannot be defined using `on_action()`. It also does not take the `options` argument.
>
> Its `fn` closure may take up to two arguments: the [Runtime Instance](#l--rt.var) (typically named `rt`) and the [Install Requirements](#l--action.install.requirements) (typically named `reqs`).
>
> If it is not defined, it defaults to the equivalent of:
> ```rhai
> rt.action(app, "start");
> ```

> l[action.install.requirements]
> The Install Action can define special parameters which are only requested from the operator when installation is requested. The values of the Requirements are only known to Beset for the duration of the Install process, and are discarded afterwards.
>
> The Requirements Definition (the second argument to `on_install()`) is an object map of requirement key => definition.
>
> The Requirements Object (the second argument of the `fn` _closure_) is an object map of requirement key => string value.
>
> The definition has these fields:
> - `kind` (optional): how to present/validate the field, defaults to `"text"`;
> - `required` (optional): boolean, defaults to `true`;
> - `description` (optional): free-form text, for the operator to understand what the required value is or is for;
> - `default_value` (optional): string, the value to use if none is provided.
> 
> If `default_value` is set and `required` is `true`, then the default value is pre-populated in the field input (but the field is still mandatory / cannot be submitted empty).

> l[action.install.requirements.kind-text]
> A requirement kind of `"text"` is a free-form text field.
> No validation is applied.

> l[action.install.requirements.kind-email]
> A requirement kind of `"email"` is an email address field.
> Basic validation is applied, and hints may be provided for more outlandish values ([for example](https://en.wikipedia.org/wiki/Email_address#Valid_email_addresses), `" "@example.org` is a valid email address, but probably not what an user meant).

> l[action.install.requirements.kind-password]
> A requirement kind of `"password"` is a strong password field.
> Weak passwords must not be accepted (what makes a weak password is implementation-defined, but should be something like [zxcvbn](https://lowe.github.io/tryzxcvbn/)).
> The field should have a strong password generator available.

> l[action.install.requirements.kind-weak-password]
> A requirement kind of `"weak-password"` is a free-form password field.
> Password strength should be hinted, but must not restrict submission.
> The field should have a strong password generator available.

# Runtime Instance

The Runtime Instance is a handle to the Beset runtime for an application.
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

> l[rt.scheduled]
> `Scheduled` is an opaque type representing a Collection of scheduled resources.

> l[rt.lifecyle]
> - _Scheduling_: the resource is set up "in the world" (on the node or cluster).
> - _Ready_: the resource is ready to be used.
> - _Terminating_: termination has been initiated by the runtime.
> - _Terminated_: the resource has terminated.
> - _Unscheduled_: the resource has been cleaned up.
>
> It's the runtime's concern as to how the scheduling and other lifecycle actions and events work for each resource type.
>
> Note that a resource can transition directly from _Ready_ to _Terminated_, for example when it exits on its own.

## Workload control

> l[rt.start]
> The `rt.start(resources: Collection)` method schedules the resources in the Collection and blocks until all become ready. It returns a [Scheduled](#l--rt.scheduled).
>
> Ordering is undefined within the Collection.

> l[rt.stop]
> The `rt.stop(resources: Collection)` method unschedules the resources in the Collection and blocks until all terminate.
>
> Ordering is undefined within the Collection.

> l[rt.run]
> The `rt.run(resources: Collection)` method schedules the resources in the Collection and blocks until all terminate.
>
> Ordering is undefined within the Collection.
>
> This is primarily useful for [Jobs](#l--job.type), which are expected to terminate.

> l[rt.wait]
> The `rt.wait(resources: Scheduled)` method blocks until the [Scheduled](#l--rt.scheduled) terminates.
>
> Ordering is undefined for the scheduled resources.
>
> This is primarily useful for [Jobs](#l--job.type), to be able to [start](#l--rt.start) a Job and then do other things while it's running, and then synchronise on it ending.

> l[rt.action]
> The `rt.action(app: App, name: string)` method invokes an Action, and blocks until it completes.

> l[rt.reconcile]
> The `rt.reconcile(old: Resource, new: Resource)` method converts one Resource into another, and blocks until that process is done.
>
> How exactly that happens is defined by the runtime (not in this spec).
> Non-normatively, an example is reconciling an [Ingress](#l--ingress.type) into another, which will happen without dropping traffic.
>
> If a reconciliation is not implemented for the pair of resources, this is equivalent to:
> ```rhai
> rt.stop(old);
> rt.start(new);
> ```
>
> Note that this does not support Collections, it's specifically one Resource to one Resource.
