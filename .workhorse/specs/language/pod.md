---
id: POD
---

# Pods

Applies to the BSL `Pod` interface: the builder methods and semantics every workload with its own network namespace shares.

## Pod interface

`Pod` is an interface, and no value has the type `Pod`.
A pod has its own network namespace and holds one container.
Every type that implements `Pod` also implements [`Container`](container.md).
[Deployments](deployment.md) and [Jobs](job.md) implement `Pod`.
Every method in this spec is a [builder method](rule:l/bsl.builder) available on each type that implements `Pod`.

## Service mounts

`pod.mount(svc: ServicePort)` makes the [Service Port](rule:l/service.port) `svc` reachable from inside the pod.
The runtime MUST make a mounted Service Port reachable at the hostname `localmount`, on the Service Port's port number, as [pod service mounts](rule:r/infra.pod.mount) specifies.

## Port bindings

A script binds a port of the pod to a service with these methods, where `port` is a [port number](rule:l/bsl.port) inside the pod:

| method | binds `port` to | equivalent to |
| --- | --- | --- |
| `pod.http(port: number, svc: HttpServiceRoute)` | the [route](http-service.md#routes) `svc` | |
| `pod.http(port: number, svc: HttpService)` | the route `/` of `svc` | `http(port, svc.route("/"))` |
| `pod.tcp(port: number, svc: ServicePort)` | the Service Port `svc`, for TCP | |
| `pod.tcp(port: number, svc: Service)` | port `port` of `svc`, for TCP | `tcp(port, svc.port(port))` |
| `pod.udp(port: number, svc: ServicePort)` | the Service Port `svc`, for UDP | |
| `pod.udp(port: number, svc: Service)` | port `port` of `svc`, for UDP | `udp(port, svc.port(port))` |

The runtime MUST route traffic of the bound protocol arriving at the route or Service Port to `port` on the pod, and its replies back.

Binding a pod to a route declared as a [redirect](http-service.md#redirect-route-constraints) MUST throw.
