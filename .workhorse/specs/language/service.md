---
id: SVC
---

# Services

Applies to services: network endpoints that an [app](../overview.md#app) declares, or that an [operator](../overview.md#operator) supplies to it, through which its workloads and operators reach one another.

## Terminology

### Service port

A port of a service at which a client connects to the service.

### External service

A service an app declares by name and whose endpoint an operator supplies.

## Declaration

`app.service(name: string)` MUST return a `Service` [builder](overview.md#builder) for the named service.
Anonymous services are specified in [APP](app.md#anonymous-resources).

## Service ports

`service.port(port: number)` is an [instance method](overview.md#instance-method) that MUST return a `ServicePort` for the service port numbered `port`.
`port` follows the [port rules](overview.md#ports).

A service port's number is the port at which a client connects to the service.
A [pod binding](rule:l/pod.tcp) MAY attach it to a pod port with a different number.

## Routing

The runtime MUST deliver TCP and UDP traffic addressed to a service port to a target bound to that port.
If a service port has no target, the runtime MUST drop or reject traffic addressed to it.
Where a service port has several targets, the runtime SHOULD distribute traffic among them round-robin.

The runtime MUST distribute HTTP traffic that reaches a service through an [ingress](rule:l/ingress.type) under the service's [balancing policy](rule:l/service.balance).

## Export

`service.exported()` and `service.exported(options: map)` are builder methods that MUST mark the service as exported.
`options` MAY hold `description`, a string describing the service to operators.

Only a named static service can be exported.
`exported` MUST throw when called on an anonymous service.

The runtime MUST list each exported service to operators, with its export description where it has one.

## External services

`app.external_service(name: string)` MUST return an `ExternalService` for the named external service.
An `ExternalService` MUST offer the methods `port`, `http`, `balance` and `description`, with the meanings they have on a `Service`.
A script MAY [mount](rule:l/pod.mount-serviceport) or bind an external service's ports as for any service.

The runtime MUST route traffic for an external service to the endpoint an operator has mapped to it.
An external service with no mapping has no target.
Changes to the mapping are specified in [external service mappings](rule:r/service.external.mapping.events).

In the [action context](app.md#action-context), `app.external_service(name)` MUST return a reference to the static external service of that name, and MUST throw if there is none, as for every [named resource](app.md#named-resources).
The reference is immutable like every static resource, so `description`, `balance` and `http` MUST throw on it.
The external services an app declares are therefore exactly those its static context declares.
