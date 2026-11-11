---
id: POD
---

# Pods

## Pod

<!-- from language: Pod -->

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
> The `pod.http(port: number, svc: HttpServiceRoute)` or `pod.http(port: number, svc: HttpService)` builder method attaches a `port` of the pod to an [HTTP Service Route](rule:l/service.http.route).
> The `http(port, svc: HttpService)` form is equivalent to `http(port, svc.route("/"))`.
>
> HTTP traffic to the HTTP Service Route will be routed to (and back from) the port on the pod.

> l[pod.tcp]
> The `pod.tcp(port: number, svc: ServicePort)` or `pod.tcp(port: number, svc: Service)` builder method attaches a `port` of the pod to a [Service Port](rule:l/service.port).
> The `tcp(port, svc: Service)` form is equivalent to `tcp(port, svc.port(port))`.
>
> TCP traffic to the Service Port will be routed to (and back from) the port on the pod.

> l[pod.udp]
> The `pod.udp(port: number, svc: ServicePort)` or `pod.udp(port: number, svc: Service)` builder method attaches a `port` of the pod to a [Service Port](rule:l/service.port).
> The `udp(port, svc: Service)` form is equivalent to `udp(port, svc.port(port))`.
>
> UDP traffic to the Service Port will be routed to (and back from) the port on the pod.
