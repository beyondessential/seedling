---
id: PRX
---

# Proxy

## Resource Lifecycle Semantics

<!-- from runtime: Lifecycle / Resource Lifecycle Semantics -->

> r[service.http.route.routing]
> URL-prefix routing is a property of the Service. When an HTTP-terminating Ingress (whether declared by the service's own app or attached as a [site ingress](rule:r/ingress.site.attachment)) fronts a Service whose backing pods declare per-prefix [HTTP route](rule:l/service.http.route) bindings, the runtime must route each request to the pods that bound the matching URL prefix, not to the Service's general routing pool.
>
> Concretely: for every `deployment.http(pod_port, svc.route(prefix))` binding on a pod backing the ingress's target service, the runtime emits one ingress route at `prefix` with upstreams set to the running backend pods that hold that binding. The proxy must select the longest matching prefix for each incoming request.
>
> When the backing service has no `http_bindings` at all (e.g. an HTTPS-fronted TCP-only service), the runtime falls back to a single `/` route through the Service's general routing pool.
> A [redirect route](rule:r/service.http.route.redirect) is declared on the Service rather than bound by a pod, and does not stand in for that fallback: a Service with no bindings takes it whether or not it also declares a redirect, so the redirect answers for its own prefix and the fallback answers for the rest.

> r[service.http.route.redirect]
> A route the app declared as a [redirect](rule:l/service.http.route.redirect) must be emitted at its prefix as a route answering with the declared status code and a `Location` built from the declared target, rather than as a reverse proxy to any pod.
>
> A redirect route is declared on the Service and does not depend on a pod binding it, unlike the proxied routes alongside it.
> A Service whose only routes are redirects must therefore emit those routes, alongside whatever else it is served through.
>
> The target's `<tail>` is the request path with the matched prefix removed, and its `<query>` the request's query string; a target naming neither is served as it was declared.
> Both are taken from the request as it arrived, so what a redirect carries over is not disturbed by any [header operation](rule:r/service.http.route.headers) the route applies to the response.
>
> Redirect routes take their place in the [longest-prefix ordering](rule:r/service.http.route.routing) alongside proxied ones, and are terminal as those are.
> A redirect on a longer prefix therefore answers for its own traffic while a proxied route on a shorter prefix serves the rest, which is what lets one path within a hostname redirect while the hostname otherwise proxies.
>
> A redirect route claims its prefix on segment boundaries, unlike a proxied one: it claims the prefix exactly, and paths below it, and nothing that merely begins with the same characters.
> The tail reaches a client as part of a URL, so a request the prefix only spells the start of would contribute a tail that is not a path — and against a target naming another host, one the client chose rather than the app.
> Matching, and the tail and query taken from it, must agree on which requests were claimed and must not disagree over the case of the request, so that a request the route answers is one the target was built from.
>
> A tail and a query are taken from the request line as it arrived, in the form it arrived in, so that what reaches the client is what the client sent rather than a re-encoding of it.
> A request carrying text that the proxy would read as naming its own state contributes no tail and no query, the same as one the prefix does not claim: what a client writes must never reach the place a [header value](rule:l/service.http.headers.fields) refuses a brace for.
>
> On the plaintext virtual host of an ingress declaring an [HTTP redirect](rule:l/ingress.redirect), that redirect answers every request whatever its path, and a redirect route must not displace it.
> A plaintext request under a redirect route's prefix is therefore sent to HTTPS first and meets the redirect route on arrival.
> That costs a further round trip, and is what stops a client being handed a target it would reach without ever having been moved to TLS.

> r[service.http.route.compression]
> Every reverse-proxy route the runtime emits must compress the responses it serves, according to that route's resolved [compression settings](rule:l/service.http.compress).
> Compression is applied by the proxy, so a pod need not implement it to benefit from it.
>
> Compression applies to reverse-proxy routes alone.
> Redirect responses, whether from an app-declared [ingress redirect](rule:l/ingress.redirect),
> a [route redirect](rule:r/service.http.route.redirect), or a [site ingress redirect attachment](rule:r/ingress.site.attachment), and non-HTTP forwarding must be emitted without it.
>
> A response that already carries a content encoding chosen by the upstream must be forwarded as it stands, rather than compressed a second time.

> r[service.http.route.balancing]
> Every reverse-proxy route the runtime emits must distribute requests across its upstreams according to that route's resolved [balancing settings](rule:l/service.balance).
>
> Balancing applies to reverse-proxy routes alone.
> Redirect responses and non-HTTP forwarding have no pool of upstreams to choose from, and must be emitted without it.
> A route with a single upstream has no choice to make either, so its policy has no observable effect there; this is the case for the fallback `/` route of a service with no HTTP route bindings, whose try duration and interval still apply.
>
> Within the route's try duration, a request whose connection to an upstream cannot be established must be attempted against another upstream from the route's pool, pausing for the route's interval between attempts.
> This must happen whatever the request method, because no part of a request that never connected can have been acted on.
> The effect is that a request arriving while an instance is shutting down is served by a surviving instance instead of failing, so a [rolling update](rule:r/update.rolling) does not surface as errors to clients.
>
> A request whose connection was established but which then yielded no response, because the upstream closed the connection or fell silent, must be attempted again only when it is a `GET`, which alone can be repeated without risk of acting twice.
> A response that does arrive carrying an error status is the upstream's answer rather than a failure to reach it, and must be returned to the client whatever that status.
>
> The proxy must not form its own opinion of upstream health.
> The upstream list it is given is authoritative: which backends are eligible is decided by the [routing pool](rule:r/lifecycle.service.routing-pool), from the runtime's own [healthchecks](rule:l/deployment.healthcheck).
> Health checking in the proxy would contradict that pool, which deliberately keeps every running backend in service when no healthy alternative exists rather than reducing capacity to nothing.
> Retrying an unreachable upstream covers the interval between a backend becoming unusable and the pool being recomputed on a subsequent tick; it is not a substitute for the pool.

> r[service.http.route.rate-limiting]
> A reverse-proxy route the runtime emits with a resolved [rate limit](rule:l/service.http.rate-limit) must reject a client's requests that exceed that limit before they are proxied, so the excess never reaches a backend.
>
> Rate limiting applies to reverse-proxy routes alone.
> Redirect responses and non-HTTP forwarding are emitted without it, as is any route whose resolved limit is absent.
>
> A limit belongs to the route that declared it, so a client has one budget for that route however many virtual hosts front it: a service reachable through more than one hostname, or through both a plaintext and a TLS ingress, does not thereby grant a client a budget per route of entry.
>
> A client is identified by the IP address the proxy attributes to the request, with all addresses in a single IPv6 /64 counted as one client, since a party holds its whole /64 and would otherwise hold a budget for every address in it.
> IPv4 addresses are counted individually.
> Each client may make the route's `max_events` requests within its `window`, measured as a sliding window; a request beyond that is answered with 429 and a Retry-After indicating when the client may retry.
>
> Because routes are emitted [longest-prefix-first](rule:r/service.http.route.routing) and are terminal, each request is limited by exactly one route: a request under a longer, more specific prefix is counted only against that prefix's limit, not also against a shorter prefix that would otherwise match.
> A tighter limit on a longer prefix therefore governs its own traffic independently of a looser limit on a shorter prefix covering the rest, which is what lets a login prefix carry a stricter limit than the API prefix enclosing it.
> A prefix is matched against a normalised form of the request path: duplicate separators collapsed, relative segments resolved, letter case folded, and percent-encoding decoded.
> Requests differing only in those respects are therefore counted against the same route, and a stricter limit on a nested prefix cannot be evaded by varying them.
> Normalisation is the proxy's own and may be broader than a backend's, so where the two disagree a request is counted against the more specific prefix while the backend treats it as belonging to a less specific one — the direction that over-applies the stricter limit rather than escaping it.

> r[service.http.route.headers]
> Every reverse-proxy route the runtime emits must apply its resolved [header operations](rule:l/service.http.headers) to the traffic it carries: the request operations to each request before it is proxied, and the response operations to each response before it is returned to the client.
>
> A request operation must reach the upstream, so a route replacing `Host` addresses its pods by the name the operation set, whatever hostname the client used to reach the ingress.
> `Host` names the message's destination rather than travelling as an ordinary header, so a route that appears to set it while the pods still observe the client's hostname has not applied the operation at all.
>
> A response operation must be applied to every response the route serves, whether the upstream produced it or the proxy produced it in the upstream's place: a [rate-limit rejection](rule:r/service.http.route.rate-limiting), or a failure to reach any upstream.
> A route declaring that nothing under it may be cached would otherwise go unhonoured for exactly the responses least worth caching.
>
> Operations must be applied as declared, without interpretation.
> A header carrying several values must reach the other side carrying several, rather than folded into one.
>
> A [route redirect](rule:r/service.http.route.redirect) is a response the route serves, so the route's response operations must be applied to it by the same rule.
> Its `Location` must be the one the redirect computed: the route carries no response operation naming that header, so none can displace it.
>
> Request operations apply to reverse-proxy routes alone, there being no onward request anywhere else.
> Ingress and site-ingress redirect responses and non-HTTP forwarding must be emitted without header operations of either direction.

> r[service.http.route.proxy-settings.visibility]
> The compression, balancing, rate-limit, and header settings in force on a service and on each of its routes, after resolution, must be readable when inspecting the app that declares the service, as [app.describe.proxy-settings](rule:i/app.describe.proxy-settings) defines.
> An operator diagnosing an uncompressed response, a failed request, an unexpected 429, or a header the pods did not send can then establish what the proxy was told to do without reading the app's script.
>
> These settings are declared by the app. The runtime provides no means to change them on a running app.

## Actuation

<!-- from runtime: Actuation -->

> r[ingress.persistent-connections]
> An HTTP/1.1 connection to an ingress must be persistent: the proxy must serve successive requests on the connection it was given rather than closing it after each exchange, and must hold an idle connection open long enough for a client to reuse it.
>
> This is a property of the platform rather than something an app declares.
> HTTP/2 and HTTP/3 carry many requests over one connection by their own design, and a persistent HTTP/1.1 connection reaches the same reuse; a client is not made to pay a connection per request for speaking the older protocol.
>
> The proxy must reach this without emitting connection-management headers of its own onto responses, which are forbidden on HTTP/2 and HTTP/3 and would render a response there ill-formed.
> An app therefore never needs to ask for keep-alive, and [cannot declare it](rule:l/service.http.headers.fields) as a header operation.

## Infrastructure

<!-- from runtime: Infrastructure -->

> r[infra.proxy.startup]
> The runtime attempts to bring the network proxy up at startup and, on each reconciliation
> tick, verifies its health and restarts it if necessary.
> A failure to bring the proxy up must not prevent the runtime from starting: the operator
> interface, database, scheduler, and reconciliation loop must come up regardless. The network
> proxy is workload-ingress infrastructure; its unavailability degrades ingress but not the
> control plane. When the initial bring-up fails the runtime files a system fault and the
> reconciliation loop retries on subsequent ticks, clearing the fault once the proxy is healthy.

> r[infra.proxy.image.modules]
> The proxy image the runtime pulls must provide every proxy module that the configuration
> the runtime emits can reference. The proxy rejects a configuration that names a module it
> does not provide, so a missing module does not degrade a single route: it makes the whole
> configuration unappliable, aborting the upgrade under
> [infra.proxy.upgrade.rollback](rule:r/infra.proxy.upgrade.rollback) and leaving the proxy
> serving its previous configuration indefinitely.
> The runtime must declare the set of modules it requires, so that an image can be checked
> against what the runtime actually emits rather than against a separately maintained list.

> r[infra.proxy.image.config]
> The proxy image must accept, in full, the configuration the runtime emits for every
> feature it can emit. The proxy decodes each module's configuration strictly, so a field
> the image's version of a module does not declare fails the whole document rather than the
> route that carried it, and every ingress on the node loses service together.
> [infra.proxy.image.modules](rule:r/infra.proxy.image.modules) does not cover this: a module
> can be present and still not declare a field the runtime emits.
>
> The emitted configuration must therefore be checked against the image itself, using the
> proxy's own validation, before the image is published and whenever the emitter changes.
> The check must be one that can fail: it must be shown to reject a configuration carrying
> a field the image does not declare.

> r[infra.proxy.image.cert-cache]
> The proxy image must store its certificate cache at the location the runtime observes it
> at. The certificate status in [observe.ingress.certs](rule:r/observe.ingress.certs) and the
> default-strategy metadata in [tls.cert.metadata](rule:r/tls.cert.metadata) are both derived
> from that cache, and a proxy that obtains certificates but writes them elsewhere satisfies
> neither: it serves traffic correctly while the runtime reports that no certificate exists,
> which in turn stalls every [`rt.warm_certs`](rule:l/rt.warm-certs) barrier waiting on it.

> r[infra.proxy.upgrade]
> When the configured proxy image digest differs from the running container's image digest,
> the runtime must upgrade the proxy using a blue/green strategy: the replacement container
> is started and fully configured before any traffic is directed to it. The name of the
> currently-active proxy container is persisted in the database so that a crash at any
> point in the upgrade sequence can be recovered deterministically on the next startup.

> r[infra.proxy.upgrade.cache]
> The runtime must persist the last successfully applied proxy configuration so that it can
> be applied to a replacement container during an upgrade before the traffic cutover occurs,
> ensuring no window exists where the new container receives traffic without a valid
> configuration. The persisted configuration must be stored in a form the runtime owns and
> can re-render — not a form whose validity depends on the version of the network proxy
> running at cache time — so that an upgrade that brings in a breaking change in the proxy's
> configuration format does not invalidate the cached value.

> r[infra.proxy.upgrade.rollback]
> If applying the persisted proxy configuration to a replacement container during an upgrade
> fails (for example, because the new container rejects the configuration as malformed), the
> runtime must abort the upgrade: it must stop the replacement container, leave the previous
> container running and authoritative, and not commit the cutover. The runtime must report
> the failure and retry the upgrade on a subsequent reconciliation tick.
