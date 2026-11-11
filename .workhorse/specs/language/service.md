---
id: SVC
---

# Services

## Service

<!-- from language: Service -->

> l[service.type]
> A Service is a network endpoint that operators and other resources can access.
> 
> Services are defined using the `app.service(name: string)` method, which returns a [builder](rule:l/bsl.builder).

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
> HTTP traffic reaching the Service through an Ingress is distributed by the proxy under the route's [balancing policy](rule:l/service.balance), which is round-robin unless the app selects otherwise.

## HTTP Service

<!-- from language: Service / HTTP Service -->

> l[service.exported]
> `service.exported(options?: #{ description?: string })` is a builder method which marks the service as exported. Exported services are advertised to the control plane and operators.
>
> Only named static services can be exported. Calling `exported()` on an anonymous service must throw.

## External Service

<!-- from language: Service / External Service -->

> l[service.external]
> An External Service is a Service provided by the Seedling control plane to a BSL script, at a particular name.
>
> External Services are defined using the `app.external_service(name: string)` method, which returns an `ExternalService`.
>
> External Services can't be modified or configured further, only [mounted](rule:l/container.mount-service) or [bound](rule:l/pod.bind) like any other Service. The concrete endpoint the slot resolves to is supplied by the operator via a mapping; see the runtime spec [service.external.mapping.events](rule:r/service.external.mapping.events) for the mapping lifecycle.

> l[service.external.action-context]
> In the action context, `app.external_service(name)` returns a reference to the static External Service of that name, and throws if there is none, as [app.resources.context.named](rule:l/app.resources.context.named) requires of every named resource.
> The reference is immutable like any other static resource: `description`, `balance` and `http` throw on it, however the handle was obtained.
> No External Service is created in the action context, so the set of slots an operator maps is the set the static script declares.
