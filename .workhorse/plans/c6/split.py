#!/usr/bin/env python3
"""Split docs/spec/*.md into .workhorse/specs/ per the map below.

Rule markers are kept; links to rules become `rule:<prefix>/<id>` placeholders.
"""

import re
import sys
from collections import OrderedDict
from pathlib import Path

ROOT = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
SRC = ROOT / "docs/spec"
DST = ROOT / ".workhorse/specs"

PREFIX = {"language": "l", "runtime": "r", "interface": "i", "web": "w"}

TARGETS = {
    # language
    "language/overview.md": ("BSL", "BES Seedling Language"),
    "language/bundle.md": ("BND", "Definition bundles"),
    "language/collection.md": ("COL", "Collections"),
    "language/constants.md": ("CONST", "Constants"),
    "language/app.md": ("APP", "The app global"),
    "language/parameter.md": ("PARAM", "Parameters"),
    "language/service.md": ("SVC", "Services"),
    "language/http-service.md": ("HTTP", "HTTP services"),
    "language/ingress.md": ("ING", "Ingresses"),
    "language/deployment.md": ("DEP", "Deployments"),
    "language/job.md": ("JOB", "Jobs"),
    "language/container.md": ("CTR", "Containers"),
    "language/pod.md": ("POD", "Pods"),
    "language/volume.md": ("VOL", "Volumes"),
    "language/action.md": ("ACN", "Actions"),
    "language/rt.md": ("RT", "The runtime handle"),
    # runtime
    "runtime/overview.md": ("RUN", "Runtime"),
    "runtime/reconciliation.md": ("RCN", "Reconciliation"),
    "runtime/engine-limits.md": ("LIM", "Script engine limits"),
    "runtime/generation.md": ("GEN", "Generations"),
    "runtime/app-teardown.md": ("TRD", "App teardown"),
    "runtime/definition-recheck.md": ("RCK", "Definition re-checks"),
    "runtime/lifecycle.md": ("LCY", "Resource lifecycle"),
    "runtime/proxy.md": ("PRX", "Proxy"),
    "runtime/history.md": ("HIST", "Persistent history"),
    "runtime/audit-log.md": ("AUD", "Audit log"),
    "runtime/garbage-collection.md": ("GC", "Garbage collection"),
    "runtime/operations.md": ("OPS", "Lifecycle operations"),
    "runtime/scheduled-actions.md": ("SCHED", "Scheduled actions"),
    "runtime/suspension.md": ("SUSP", "Action closure suspension and replay"),
    "runtime/autonomous.md": ("AUTO", "Autonomous reconciliation"),
    "runtime/observation.md": ("OBS", "Observation"),
    "runtime/actuation.md": ("ACTU", "Actuation"),
    "runtime/images.md": ("IMG", "Images"),
    "runtime/site-resources.md": ("SITE", "Site resources"),
    "runtime/priority.md": ("PRIO", "Resource priority"),
    "runtime/update-strategies.md": ("UPD", "Update strategies"),
    "runtime/faults.md": ("FLT", "Faults"),
    "runtime/identity.md": ("IDN", "Resource identity"),
    "runtime/infrastructure.md": ("INFRA", "Host infrastructure"),
    "runtime/resolver.md": ("DNS", "Resolver"),
    "runtime/nat64.md": ("NAT64", "NAT64"),
    "runtime/secrets.md": ("SEC", "Secret parameter storage"),
    "runtime/tls/certificates.md": ("TLS", "TLS certificates"),
    "runtime/tls/chains.md": ("CHAIN", "TLS strategy chains"),
    "runtime/tls/canopy.md": ("TCAN", "Canopy certificate issuance"),
    "runtime/canopy-reporting.md": ("CNR", "Canopy reporting"),
    # interface
    "interface/overview.md": ("OI", "Operator Interface"),
    "interface/transport.md": ("TRN", "Transport"),
    "interface/wire.md": ("WIRE", "Streams and wire format"),
    "interface/status.md": ("STS", "Host status"),
    "interface/apps.md": ("APPS", "App management"),
    "interface/definitions.md": ("DEFN", "App definitions"),
    "interface/app-control.md": ("CTL", "App control"),
    "interface/params.md": ("PRM", "Parameter management"),
    "interface/actions.md": ("ACI", "Action invocation"),
    "interface/shells.md": ("SHL", "Shell sessions"),
    "interface/port-forwards.md": ("FWD", "Port forwards"),
    "interface/logs.md": ("LOG", "Log streaming"),
    "interface/faults.md": ("FLTS", "Faults and restarts"),
    "interface/events.md": ("EVT", "Event feed"),
    "interface/keys.md": ("KEY", "Key management"),
    "interface/images.md": ("IMGM", "Image management"),
    "interface/templates.md": ("TPL", "Templates"),
    "interface/tls.md": ("TLSM", "TLS certificate management"),
    "interface/canopy-relay.md": ("RLY", "Canopy relay"),
    "interface/client.md": ("CLI", "Client behaviour"),
    # web
    "web/serving.md": ("WEB", "Web interface serving"),
    "web/console.md": ("UI", "Web console"),
}

# (source, top heading, sub heading or None) -> target. Sub heading None matches any sub heading not listed.
SECTIONS = {
    ("language", None, None): "language/overview.md",
    ("language", "Collection", None): "language/collection.md",
    ("language", "Constants", None): "language/constants.md",
    ("language", "App global", None): "language/app.md",
    ("language", "App global", "Bundle files"): "language/bundle.md",
    ("language", "Parameter", None): "language/parameter.md",
    ("language", "Service", None): "language/service.md",
    ("language", "Service", "HTTP Service"): "language/http-service.md",
    ("language", "Ingress", None): "language/ingress.md",
    ("language", "Deployment", None): "language/deployment.md",
    ("language", "Job", None): "language/job.md",
    ("language", "Container", None): "language/container.md",
    ("language", "Pod", None): "language/pod.md",
    ("language", "Volume", None): "language/volume.md",
    ("language", "Action", None): "language/action.md",
    ("language", "Runtime Instance", None): "language/rt.md",
    ("runtime", None, None): "runtime/overview.md",
    ("runtime", "Reconciliation", None): "runtime/reconciliation.md",
    ("runtime", "Script Engine Limits", None): "runtime/engine-limits.md",
    ("runtime", "Desired State", None): "runtime/reconciliation.md",
    ("runtime", "Generation", None): "runtime/generation.md",
    ("runtime", "Definition Sources", None): "runtime/definition-recheck.md",
    ("runtime", "Lifecycle", None): "runtime/lifecycle.md",
    ("runtime", "Persistent History", None): "runtime/history.md",
    ("runtime", "Audit Log", None): "runtime/audit-log.md",
    ("runtime", "Garbage Collection", None): "runtime/garbage-collection.md",
    ("runtime", "Lifecycle Operations", None): "runtime/operations.md",
    ("runtime", "Scheduled Actions", None): "runtime/scheduled-actions.md",
    ("runtime", "Action Closure Suspension", None): "runtime/suspension.md",
    ("runtime", "Autonomous Reconciliation", None): "runtime/autonomous.md",
    ("runtime", "Observation", None): "runtime/observation.md",
    ("runtime", "Actuation", None): "runtime/actuation.md",
    ("runtime", "Resource Priority", None): "runtime/priority.md",
    ("runtime", "Update Strategies", None): "runtime/update-strategies.md",
    ("runtime", "Fault Handling", None): "runtime/faults.md",
    ("runtime", "Resource Identity", None): "runtime/identity.md",
    ("runtime", "Startup", None): "runtime/actuation.md",
    ("runtime", "Infrastructure", None): "runtime/infrastructure.md",
    ("runtime", "Infrastructure", "Resolver"): "runtime/resolver.md",
    ("runtime", "Infrastructure", "Container DNS"): "runtime/resolver.md",
    ("runtime", "Infrastructure", "NAT64"): "runtime/nat64.md",
    ("runtime", "Secret Parameter Storage", None): "runtime/secrets.md",
    ("runtime", "TLS Certificate Strategy", None): "runtime/tls/certificates.md",
    ("runtime", "TLS Certificate Strategy", "Strategy chains"): "runtime/tls/chains.md",
    ("runtime", "TLS Certificate Strategy", "Canopy"): "runtime/tls/canopy.md",
    ("runtime", "Canopy Reporting", None): "runtime/canopy-reporting.md",
    ("interface", None, None): "interface/overview.md",
    ("interface", "Transport", None): "interface/transport.md",
    ("interface", "Streams", None): "interface/wire.md",
    ("interface", "Wire Format", None): "interface/wire.md",
    ("interface", "Status", None): "interface/status.md",
    ("interface", "App Management", None): "interface/apps.md",
    ("interface", "App Definitions", None): "interface/definitions.md",
    ("interface", "App Status", None): "interface/apps.md",
    ("interface", "App Description", None): "interface/apps.md",
    ("interface", "Scaling", None): "interface/app-control.md",
    ("interface", "App Priority", None): "interface/app-control.md",
    ("interface", "Deployment Restart", None): "interface/app-control.md",
    ("interface", "Resource Stop / Unstop", None): "interface/app-control.md",
    ("interface", "App Script Retrieval", None): "interface/apps.md",
    ("interface", "Generation History", None): "interface/apps.md",
    ("interface", "Change Planning", None): "interface/params.md",
    ("interface", "Param Management", None): "interface/params.md",
    ("interface", "Action Invocation", None): "interface/actions.md",
    ("interface", "Shell Sessions", None): "interface/shells.md",
    ("interface", "Volume Shells", None): "interface/shells.md",
    ("interface", "Port Forwards", None): "interface/port-forwards.md",
    ("interface", "Log Streaming", None): "interface/logs.md",
    ("interface", "Fault Surface", None): "interface/faults.md",
    ("interface", "Restart Surface", None): "interface/faults.md",
    ("interface", "Event Feed", None): "interface/events.md",
    ("interface", "Key Management", None): "interface/keys.md",
    ("interface", "Key Management", "Registry Allowlist"): "interface/images.md",
    ("interface", "Key Management", "Image Management"): "interface/images.md",
    ("interface", "Templates", None): "interface/templates.md",
    ("interface", "TLS Certificate Management", None): "interface/tls.md",
    ("interface", "Canopy Relay", None): "interface/canopy-relay.md",
    ("interface", "Client Behaviour", None): "interface/client.md",
    ("web", None, None): "web/serving.md",
    ("web", "Transport", None): "web/serving.md",
    ("web", "Authentication", None): "web/serving.md",
    ("web", "WebTransport Session", None): "web/serving.md",
    ("web", "SPA Delivery", None): "web/serving.md",
    ("web", "Operator Capabilities", None): "web/console.md",
    ("web", "Bind Configuration", None): "web/serving.md",
}

# Rules placed by subject rather than by the section they sat in.
RULES = {
    "l/bsl.bundle": "language/bundle.md",
    "l/bsl.bundle.metadata": "language/bundle.md",
    "l/bsl.bundle.seedling-versions": "language/bundle.md",
    "l/bsl.bundle.script-errors": "language/bundle.md",
    "l/service.exported": "language/service.md",
    "r/barrier.replay.positional": "runtime/suspension.md",
    "r/history.persist.partial-update": "runtime/history.md",
    "r/deregister.purge": "runtime/app-teardown.md",
    "r/service.http.route.routing": "runtime/proxy.md",
    "r/service.http.route.redirect": "runtime/proxy.md",
    "r/service.http.route.compression": "runtime/proxy.md",
    "r/service.http.route.balancing": "runtime/proxy.md",
    "r/service.http.route.rate-limiting": "runtime/proxy.md",
    "r/service.http.route.headers": "runtime/proxy.md",
    "r/service.http.route.proxy-settings.visibility": "runtime/proxy.md",
    "r/template.persist": "interface/templates.md",
    "r/rt.signal": "runtime/suspension.md",
    "r/rt.write": "runtime/suspension.md",
    "r/rt.exec": "runtime/suspension.md",
    "r/ingress.persistent-connections": "runtime/proxy.md",
    "r/namespace.reserved": "runtime/identity.md",
    "r/app.uninstall.scope": "runtime/app-teardown.md",
    "r/app.uninstall.exclusive": "runtime/app-teardown.md",
    "r/infra.pod.subnet": "runtime/infrastructure.md",
}
for rid in (
    "actuate.image.retry actuate.image.warm image.pin image.pin.expiry image.pin.update-reconcile "
    "image.track image.gc image.discover image.discover.params"
).split():
    RULES["r/" + rid] = "runtime/images.md"
for rid in (
    "volume.site volume.site.lifecycle volume.site.name-taken volume.site.lifecycle.events "
    "volume.site.snapshot volume.site.snapshot.events volume.site.snapshot.delete volume.site.promote "
    "volume.site.promote.events volume.site.leftover-snapshots volume.external.mapping.events "
    "service.site service.site.address service.site.address.partial-answer service.site.lifecycle "
    "service.site.lifecycle.events service.external.mapping.events ingress.site ingress.site.lifecycle "
    "ingress.site.lifecycle.events ingress.site.attachment ingress.site.tailscale ingress.site.conflict"
).split():
    RULES["r/" + rid] = "runtime/site-resources.md"
for rid in (
    "infra.proxy.startup infra.proxy.image.modules infra.proxy.image.config infra.proxy.image.cert-cache "
    "infra.proxy.upgrade infra.proxy.upgrade.cache infra.proxy.upgrade.rollback"
).split():
    RULES["r/" + rid] = "runtime/proxy.md"

RULE_LINE = re.compile(r"^> ([rliw])\[([a-z0-9.\-]+)\]\s*$")
HEADING = re.compile(r"^(#{1,3}) (.+?)\s*$")
LINK = re.compile(r"\]\((?:(?:\.\./)?([a-z]+)\.md)?#([rliw])--([a-z0-9.\-]+)\)")


def relink(text, own):
    def sub(m):
        return f"](rule:{m.group(2)}/{m.group(3)})"

    return LINK.sub(sub, text)


def target_for(src, top, sub, rule):
    if rule and rule in RULES:
        return RULES[rule]
    for key in ((src, top, sub), (src, top, None)):
        if key in SECTIONS:
            return SECTIONS[key]
    raise KeyError((src, top, sub, rule))


def main():
    out = OrderedDict((t, []) for t in TARGETS)
    seen = set()
    for src in PREFIX:
        lines = (SRC / f"{src}.md").read_text().splitlines()
        top = sub = None
        chunk, rule = [], None
        groups = []

        def flush():
            if any(l.strip() for l in chunk):
                groups.append((top, sub, rule, list(chunk)))

        for line in lines:
            h = HEADING.match(line)
            r = RULE_LINE.match(line)
            if h:
                flush()
                chunk, rule = [], None
                if len(h.group(1)) == 1:
                    top, sub = h.group(2), None
                else:
                    sub = h.group(2)
                continue
            if r:
                flush()
                chunk, rule = [], f"{r.group(1)}/{r.group(2)}"
                if rule in seen:
                    raise SystemExit(f"duplicate {rule}")
                seen.add(rule)
            chunk.append(line)
        flush()

        for top_, sub_, rule_, body in groups:
            t = target_for(src, top_, sub_, rule_)
            label = sub_ or top_
            out[t].append((f"{src}: {top_}" + (f" / {sub_}" if sub_ else ""), label, rule_, body))

    for t, groups in out.items():
        rid, title = TARGETS[t]
        parts = [f"---\nid: {rid}\n---\n\n# {title}\n"]
        current = object()
        for origin, label, rule, body in groups:
            if origin != current:
                parts.append(f"\n## {label or 'Preamble'}\n\n<!-- from {origin} -->\n")
                current = origin
            text = "\n".join(body).strip("\n")
            parts.append("\n" + relink(text, None) + "\n")
        path = DST / t
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("".join(parts))
    print(f"{len(seen)} rules into {len(out)} specs")


if __name__ == "__main__":
    main()
