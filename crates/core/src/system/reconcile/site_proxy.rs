use std::collections::BTreeSet;

use ipnet::Ipv6Net;
use seedling_protocol::names::{AppName, SiteIngressName};

use super::AppSnapshot;
use crate::{
    defs::{
        Port,
        ingress::{Claim, ClaimTransport, HttpTermination, IngressDef},
        resource::{Resource, ResourceKind},
    },
    runtime::{
        InstanceRegistry,
        site_ingress_attachments::{
            self, AttachmentProtocol, AttachmentTarget, SiteIngressAttachment,
        },
        site_ingresses::{self, SiteIngressDef, TlsProvider},
    },
    system::{
        translate::proxy::{RedirectTarget, ServiceUpstream},
        types::{DtlsRoute, L4Proto, L4Route},
    },
};

/// Result of resolving every site-ingress attachment against the current
/// app set + registry. Entries that couldn't be resolved (e.g. their target
/// app/service doesn't exist) appear in `missing_targets`; the reconciler
/// files faults from that set so operators see a clear error.
pub(super) struct SiteProxyData {
    pub forwards: Vec<(SiteIngressName, IngressDef, ServiceUpstream)>,
    pub redirects: Vec<(SiteIngressName, IngressDef, RedirectTarget)>,
    /// L4 (tcp/udp) forward attachments, resolved to a Caddy L4 route.
    /// Listener port comes from the attachment; upstream is the same
    /// IPv6 service address the HTTP path uses.
    // spec: SITE#attachments
    pub l4_routes: Vec<SiteL4Entry>,
    /// `(site_ingress, port, protocol, reason)` for entries that couldn't
    /// be resolved this tick. Used for fault filing.
    pub unresolved: Vec<UnresolvedAttachment>,
    /// When each site ingress first had an attachment on a port, which orders
    /// site ingresses making conflicting claims.
    // spec: SITE#ingress-conflicts
    pub created: std::collections::BTreeMap<(SiteIngressName, u16), String>,
}

#[derive(Debug, Clone)]
pub(super) struct UnresolvedAttachment {
    pub site_ingress: String,
    pub port: u16,
    pub protocol: AttachmentProtocol,
    pub reason: String,
}

/// One resolved L4 (tcp/udp) site-ingress attachment. We carry the parent
/// site ingress's name alongside the route so the conflict detector can
/// attribute collisions and the reconciler can drop both sides from the proxy
/// config when an app ingress makes a conflicting claim.
// spec: SITE#attachments
#[derive(Debug, Clone)]
pub(super) struct SiteL4Entry {
    pub site_ingress: SiteIngressName,
    pub route: L4Route,
}

/// Snapshot of the site ingress / attachment tables, loaded once per
/// reconcile tick. The collect step is a pure function over this snapshot
/// so it can run alongside the other phase computations without holding
/// the DB lock.
pub(super) struct SiteIngressSnapshot {
    pub ingresses: Vec<SiteIngressDef>,
    pub attachments: Vec<SiteIngressAttachment>,
}

/// Read the site ingresses and their attachments for one tick.
///
/// A failed read is an error, never an empty snapshot: the snapshot feeds the
/// proxy configuration and the redirect rules, which are applied wholesale, so
/// an empty one would withdraw every site ingress port.
// spec: SITE#site-ingresses, SITE#attachments
// spec: SITE#unreadable-site-ingresses
pub(super) fn load(db: &crate::runtime::db::Db) -> rusqlite::Result<SiteIngressSnapshot> {
    Ok(SiteIngressSnapshot {
        ingresses: site_ingresses::list(db)?,
        attachments: site_ingress_attachments::list_all(db)?,
    })
}

// spec: SITE#site-ingresses, SITE#attachments
pub(super) fn collect(
    snapshot: &SiteIngressSnapshot,
    apps: &[AppSnapshot],
    running_pods_by_app: &std::collections::HashMap<AppName, Vec<super::RunningPod>>,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
) -> SiteProxyData {
    let mut data = SiteProxyData {
        forwards: Vec::new(),
        redirects: Vec::new(),
        l4_routes: Vec::new(),
        unresolved: Vec::new(),
        created: std::collections::BTreeMap::new(),
    };

    for ingress in &snapshot.ingresses {
        // spec: SITE#sources discovered ingresses that have lost
        // their source (e.g. tailscaled is offline) skip serving but stay in
        // the DB so attachments survive the outage.
        if ingress.stale {
            continue;
        }
        for att in snapshot
            .attachments
            .iter()
            .filter(|a| a.site_ingress == ingress.name)
        {
            data.created
                .entry((ingress.name.clone(), att.port))
                .and_modify(|first| {
                    if att.created_at < *first {
                        first.clone_from(&att.created_at);
                    }
                })
                .or_insert_with(|| att.created_at.clone());
            resolve_attachment(
                ingress,
                att,
                apps,
                running_pods_by_app,
                node_prefix,
                registry,
                &mut data,
            );
        }
    }

    data
}

fn resolve_attachment(
    ingress: &SiteIngressDef,
    att: &SiteIngressAttachment,
    apps: &[AppSnapshot],
    running_pods_by_app: &std::collections::HashMap<AppName, Vec<super::RunningPod>>,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    data: &mut SiteProxyData,
) {
    // L4 (TCP/UDP) attachments take a separate path through Caddy's L4
    // module; redirect targets only make sense on HTTP, so a TCP/UDP
    // redirect is a configuration error.
    if matches!(
        att.protocol,
        AttachmentProtocol::Tcp | AttachmentProtocol::Udp
    ) {
        // spec: SITE#attachments
        match &att.target {
            AttachmentTarget::Forward { app, service } => {
                match resolve_forward_upstream(
                    app,
                    service,
                    att,
                    apps,
                    running_pods_by_app,
                    node_prefix,
                    registry,
                ) {
                    Ok(upstream) => {
                        let upstream_url =
                            format!("[{}]:{}", upstream.service_ip, upstream.service_port);
                        let proto = match att.protocol {
                            AttachmentProtocol::Tcp => L4Proto::Tcp,
                            AttachmentProtocol::Udp => L4Proto::Udp,
                            _ => unreachable!("guarded by outer match"),
                        };
                        data.l4_routes.push(SiteL4Entry {
                            site_ingress: ingress.name.clone(),
                            route: L4Route {
                                port: att.port,
                                proto,
                                upstreams: vec![upstream_url],
                                terminate_tls: false,
                                hostname: None,
                            },
                        });
                    }
                    Err(reason) => data.unresolved.push(UnresolvedAttachment {
                        site_ingress: ingress.name.as_str().to_owned(),
                        port: att.port,
                        protocol: att.protocol,
                        reason,
                    }),
                }
            }
            AttachmentTarget::Redirect { .. } => {
                data.unresolved.push(UnresolvedAttachment {
                    site_ingress: ingress.name.as_str().to_owned(),
                    port: att.port,
                    protocol: att.protocol,
                    reason: format!(
                        "redirect targets are only valid on HTTP/HTTP2 attachments, not {}",
                        att.protocol
                    ),
                });
            }
        }
        return;
    }

    let term = match att.protocol {
        AttachmentProtocol::Http => HttpTermination::Http1,
        AttachmentProtocol::Http2 => HttpTermination::Http2,
        AttachmentProtocol::Tcp | AttachmentProtocol::Udp => unreachable!("handled above"),
    };

    let port = match Port::new(i64::from(att.port)) {
        Ok(p) => p,
        Err(e) => {
            data.unresolved.push(UnresolvedAttachment {
                site_ingress: ingress.name.as_str().to_owned(),
                port: att.port,
                protocol: att.protocol,
                reason: format!("port {} rejected: {}", att.port, e),
            });
            return;
        }
    };

    // TLS termination is determined by the parent site ingress's TLS
    // provider. `none` means plaintext (no TLS termination); anything
    // else means Caddy terminates TLS on this listener.
    let tls = !matches!(ingress.tls_provider, TlsProvider::None);
    let def = IngressDef {
        hostname: ingress.hostname.clone(),
        port,
        tls,
        dtls: false,
        http_terminate: Some(term),
        // Site ingresses don't currently support an in-vhost HTTP→HTTPS
        // redirect; redirect attachments are independent entries on
        // their own (port, protocol).
        redirect: None,
        description: None,
    };

    match &att.target {
        AttachmentTarget::Forward { app, service } => {
            match resolve_forward_upstream(
                app,
                service,
                att,
                apps,
                running_pods_by_app,
                node_prefix,
                registry,
            ) {
                Ok(upstream) => data.forwards.push((ingress.name.clone(), def, upstream)),
                Err(reason) => data.unresolved.push(UnresolvedAttachment {
                    site_ingress: ingress.name.as_str().to_owned(),
                    port: att.port,
                    protocol: att.protocol,
                    reason,
                }),
            }
        }
        AttachmentTarget::Redirect {
            url,
            code,
            preserve_path,
        } => {
            data.redirects.push((
                ingress.name.clone(),
                def,
                RedirectTarget {
                    url: url.clone(),
                    code: *code,
                    preserve_path: *preserve_path,
                },
            ));
        }
    }
}

fn resolve_forward_upstream(
    target_app: &AppName,
    target_service: &seedling_protocol::names::AppServiceName,
    att: &SiteIngressAttachment,
    apps: &[AppSnapshot],
    running_pods_by_app: &std::collections::HashMap<AppName, Vec<super::RunningPod>>,
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
) -> Result<ServiceUpstream, String> {
    let snapshot = apps
        .iter()
        .find(|a| &a.name == target_app)
        .ok_or_else(|| format!("target app {target_app:?} is not installed"))?;

    let svc_name_str = target_service.as_str();
    let svc_resource = snapshot
        .app_def
        .resources
        .iter()
        .find(|(id, _)| id.kind == ResourceKind::Service && id.name.as_str() == svc_name_str)
        .map(|(_, r)| r);
    let Some(Resource::Service(_)) = svc_resource else {
        return Err(format!(
            "target app {target_app:?} does not declare service {svc_name_str:?}"
        ));
    };

    let svc_instance = registry
        .get_or_create_singleton(target_app, ResourceKind::Service, Some(svc_name_str))
        .map_err(|e| format!("registry lookup for {target_app}/{svc_name_str} failed: {e}"))?;
    let service_ip = registry
        .service_address(node_prefix, &svc_instance)
        .map_err(|e| format!("address lookup for {target_app}/{svc_name_str} failed: {e}"))?;

    // A service no pod binds for the attachment's transport leaves the
    // attachment out, reported as unresolved, rather than serving a 502.
    let forwarded_port = scan_apps_for_service_port(snapshot, svc_name_str, att.protocol)
        .ok_or_else(|| format!("no pod binding found for {target_app}/{svc_name_str}"))?;

    // spec: PRX#route-emission
    // Routing is a property of the *service*, not of the ingress that
    // attaches to it. A site-ingress attachment to a service with HTTP
    // route bindings must use the same per-prefix routing as an app
    // ingress to the same service — otherwise the BSL author's prefix
    // routing would silently flatten just because traffic arrived via a
    // site ingress instead of an app ingress. Routes is empty for L4
    // attachments and for HTTPS attachments fronting a TCP-only service;
    // `build_proxy_config` then falls back to a single-`/` route through
    // the service IP, which is the right behaviour in those cases.
    let empty: Vec<super::RunningPod> = Vec::new();
    let target_running: &[super::RunningPod] = running_pods_by_app
        .get(target_app)
        .map(Vec::as_slice)
        .unwrap_or(&empty);
    let service_def = super::proxy::service_proxy_def(&snapshot.app_def, svc_name_str);
    let routes = super::proxy::collect_http_routes(
        &snapshot.app_def,
        svc_name_str,
        target_running,
        &service_def,
    );
    // spec: PRX#redirect-routes
    // A redirect route is a property of the service by the same argument the
    // comment above makes for prefix routing: a path that redirects when
    // reached through the app's own ingress redirects when reached through a
    // site ingress attached to the same service.
    let redirects = super::proxy::collect_redirect_routes(&service_def);

    // spec: SITE#attachments
    // spec: PRX#route-emission
    // A TCP or UDP attachment's traffic reaches the forwarded port. An HTTP
    // attachment's routes are the service's, as an app ingress's would be, so
    // its fallback route's upstream is where an app ingress's would be.
    let service_port = match att.protocol {
        AttachmentProtocol::Tcp | AttachmentProtocol::Udp => forwarded_port,
        AttachmentProtocol::Http | AttachmentProtocol::Http2 => service_def.fallback_port(att.port),
    };

    Ok(ServiceUpstream {
        routes,
        redirects,
        service_ip,
        service_port,
        // Resolved against the app that declares the service, not the site,
        // so an attachment serves a route exactly as its own ingress would.
        proxy: super::proxy::service_level_proxy(&snapshot.app_def, svc_name_str, &service_def),
    })
}

/// The forwarded port of an attachment of `protocol` to `service_name`: the
/// service port of the first binding of it a pod of the app makes, looking in
/// the Deployments and then the Jobs, each in byte order of name.
// spec: SITE#attachments
fn scan_apps_for_service_port(
    snapshot: &AppSnapshot,
    service_name: &str,
    protocol: AttachmentProtocol,
) -> Option<u16> {
    let mut deployments = Vec::new();
    let mut jobs = Vec::new();
    for (id, resource) in &snapshot.app_def.resources {
        match resource {
            Resource::Deployment(_) => deployments.push((id.name.as_str(), resource)),
            Resource::Job(_) => jobs.push((id.name.as_str(), resource)),
            _ => {}
        }
    }
    deployments.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    jobs.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    deployments
        .into_iter()
        .chain(jobs)
        .find_map(|(_, resource)| match resource {
            Resource::Deployment(dep) => {
                let def = dep.def.lock();
                let pod = def.pod.lock();
                pod_first_port(&pod, service_name, protocol)
            }
            Resource::Job(job) => {
                let def = job.def.lock();
                let pod = def.pod.lock();
                pod_first_port(&pod, service_name, protocol)
            }
            _ => None,
        })
}

/// The service port of the pod's first binding of `service_name` for an
/// attachment of `protocol`: its port bindings of the attachment's transport,
/// in the order the script made them, then, but never for UDP, its HTTP route
/// bindings.
// spec: SITE#attachments
fn pod_first_port(
    pod: &crate::defs::pod::PodDef,
    service_name: &str,
    protocol: AttachmentProtocol,
) -> Option<u16> {
    let port_bindings = match protocol {
        AttachmentProtocol::Udp => &pod.udp_bindings,
        AttachmentProtocol::Tcp | AttachmentProtocol::Http | AttachmentProtocol::Http2 => {
            &pod.tcp_bindings
        }
    };
    if let Some(b) = port_bindings
        .iter()
        .find(|b| b.service_port.service.name().as_str() == service_name)
    {
        return Some(b.service_port.port.get());
    }
    if protocol == AttachmentProtocol::Udp {
        return None;
    }
    pod.http_bindings
        .iter()
        .find(|b| b.route.http.service.name().as_str() == service_name)
        .map(|b| b.route.http.port.get())
}

/// What the proxy serves for one app ingress.
pub(super) enum AppServe {
    /// An HTTP-terminating ingress, routed through a virtual host.
    Http(Box<ServiceUpstream>),
    /// An ingress that passes TCP through or terminates TLS without HTTP.
    Tcp(L4Route),
    /// An ingress that terminates DTLS.
    Dtls(DtlsRoute),
}

/// An app ingress, carrying its app so it can take part in conflict
/// resolution whatever it is served as.
// spec: ING#claims-across-apps
pub(super) struct AppIngress {
    pub app: AppName,
    pub def: IngressDef,
    pub serve: AppServe,
}

impl AppIngress {
    fn name(&self) -> String {
        ingress_resource_name(&self.def)
    }
}

/// An ingress's name inside its app's resource map: `<hostname>:<port>`.
/// Mirrors `crate::defs::ingress::ingress_resource_name` without depending on
/// the definitions side of the codebase here.
fn ingress_resource_name(def: &IngressDef) -> String {
    format!("{}:{}", def.hostname, def.port.get())
}

/// One party's claims, in the order the party ranks among those competing for
/// a port.
struct Candidate<O> {
    owner: O,
    claims: Vec<Claim>,
    /// The candidate this one is served with. Where that one is left out this
    /// one is too, and takes no claim away from anything else.
    rides_with: Option<usize>,
}

enum Admission {
    Served,
    /// Left out because `theirs`, a claim of the served candidate `by`,
    /// conflicts with `mine`.
    Lost {
        mine: Claim,
        theirs: Claim,
        by: usize,
    },
    /// Left out with the candidate it rides with.
    WithParent,
}

/// Serve candidates in order, leaving out each one that makes a claim
/// conflicting with one of a candidate already served, unless the two have the
/// same owner.
///
/// A conflict is between pairs of claims, so which candidates are served can
/// depend on the order. Callers rank candidates so the outcome is the same on
/// every tick.
// spec: PRX#shared-ports
fn admit<O: PartialEq>(candidates: &[Candidate<O>]) -> Vec<Admission> {
    let mut outcome: Vec<Admission> = Vec::with_capacity(candidates.len());
    for (idx, candidate) in candidates.iter().enumerate() {
        if let Some(parent) = candidate.rides_with
            && !matches!(outcome[parent], Admission::Served)
        {
            outcome.push(Admission::WithParent);
            continue;
        }
        let blocked = candidates[..idx]
            .iter()
            .zip(&outcome)
            .enumerate()
            .filter(|(_, (_, admission))| matches!(admission, Admission::Served))
            .filter(|(_, (served, _))| served.owner != candidate.owner)
            .find_map(|(by, (served, _))| {
                candidate.claims.iter().find_map(|mine| {
                    served
                        .claims
                        .iter()
                        .find(|theirs| mine.conflicts_with(theirs))
                        .map(|theirs| Admission::Lost {
                            mine: mine.clone(),
                            theirs: theirs.clone(),
                            by,
                        })
                })
            });
        outcome.push(blocked.unwrap_or(Admission::Served));
    }
    outcome
}

/// A claim of an app ingress that was left out of the proxy because another
/// app's ingress, whose app installed first, makes a conflicting claim.
// spec: ING#claims-across-apps
#[derive(Debug, Clone)]
pub(super) struct LostAppClaim {
    pub app: AppName,
    pub ingress: String,
    /// The claim is the ingress's HTTP redirect, which alone is left out, and
    /// not the ingress.
    pub redirect: bool,
    pub claim: Claim,
    /// The ingress whose claim is served, and that claim.
    pub served: ServedClaim,
}

#[derive(Debug, Clone)]
pub(super) struct ServedClaim {
    pub app: AppName,
    pub ingress: String,
    pub claim: Claim,
}

/// Keep, of the app ingresses making conflicting claims, those of the app whose
/// install completed first, and report the rest.
///
/// `install_seq` gives each app's place in the order installs completed; an
/// app with none ranks after every app with one, and the app name breaks a
/// tie, so the outcome is the same on every tick.
///
/// An ingress is left out whole, whichever of its claims lost. Its redirect is
/// a claim of its own: where only that loses, only the redirect is left out.
// spec: ING#claims-across-apps
pub(super) fn resolve_app_claims(
    mut ingresses: Vec<AppIngress>,
    install_seq: impl Fn(&AppName) -> Option<i64>,
) -> (Vec<AppIngress>, Vec<LostAppClaim>) {
    ingresses.sort_by_cached_key(|i| {
        (
            install_seq(&i.app).unwrap_or(i64::MAX),
            i.app.clone(),
            i.name(),
        )
    });

    // (index into `ingresses`, whether the candidate is its redirect)
    let mut slots: Vec<(usize, bool)> = Vec::new();
    let mut candidates: Vec<Candidate<AppName>> = Vec::new();
    for (idx, ingress) in ingresses.iter().enumerate() {
        let core = candidates.len();
        slots.push((idx, false));
        candidates.push(Candidate {
            owner: ingress.app.clone(),
            claims: ingress.def.claims(),
            rides_with: None,
        });
        if let Some(redirect) = ingress.def.redirect_claim() {
            slots.push((idx, true));
            candidates.push(Candidate {
                owner: ingress.app.clone(),
                claims: vec![redirect],
                rides_with: Some(core),
            });
        }
    }
    let outcome = admit(&candidates);

    let mut lost = Vec::new();
    let mut dropped: BTreeSet<usize> = BTreeSet::new();
    let mut redirects_dropped: BTreeSet<usize> = BTreeSet::new();
    for ((idx, redirect), admission) in slots.iter().zip(&outcome) {
        let Admission::Lost { mine, theirs, by } = admission else {
            continue;
        };
        let ingress = &ingresses[*idx];
        let (served_idx, _) = slots[*by];
        let served = &ingresses[served_idx];
        lost.push(LostAppClaim {
            app: ingress.app.clone(),
            ingress: ingress.name(),
            redirect: *redirect,
            claim: mine.clone(),
            served: ServedClaim {
                app: served.app.clone(),
                ingress: served.name(),
                claim: theirs.clone(),
            },
        });
        if *redirect {
            redirects_dropped.insert(*idx);
        } else {
            dropped.insert(*idx);
        }
    }

    let kept = ingresses
        .into_iter()
        .enumerate()
        .filter(|(idx, _)| !dropped.contains(idx))
        .map(|(idx, mut ingress)| {
            if redirects_dropped.contains(&idx) {
                ingress.def.redirect = None;
            }
            ingress
        })
        .collect();
    (kept, lost)
}

/// A site ingress attachment left out of the proxy because an attachment of
/// another site ingress, created first, makes a conflicting claim.
// spec: SITE#ingress-conflicts
#[derive(Debug, Clone)]
pub(super) struct LostSiteClaim {
    pub site_ingress: SiteIngressName,
    pub claim: Claim,
    /// The site ingress whose attachment is served, and its claim.
    pub served: SiteIngressName,
    pub served_claim: Claim,
}

/// Where a resolved site attachment is held in a [`SiteProxyData`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum SiteEntry {
    Forward(usize),
    Redirect(usize),
    L4(usize),
}

impl SiteProxyData {
    /// Every attachment held, the site ingress it belongs to, its port, and
    /// what it claims.
    fn entries(&self) -> Vec<(SiteEntry, SiteIngressName, u16, Vec<Claim>)> {
        let mut out = Vec::new();
        for (i, (name, def, _)) in self.forwards.iter().enumerate() {
            out.push((
                SiteEntry::Forward(i),
                name.clone(),
                def.port.get(),
                def.claims(),
            ));
        }
        for (i, (name, def, _)) in self.redirects.iter().enumerate() {
            out.push((
                SiteEntry::Redirect(i),
                name.clone(),
                def.port.get(),
                def.claims(),
            ));
        }
        for (i, entry) in self.l4_routes.iter().enumerate() {
            let transport = match entry.route.proto {
                L4Proto::Tcp => ClaimTransport::Tcp,
                L4Proto::Udp => ClaimTransport::Udp,
            };
            out.push((
                SiteEntry::L4(i),
                entry.site_ingress.clone(),
                entry.route.port,
                vec![Claim::whole_port(entry.route.port, transport)],
            ));
        }
        out
    }

    fn retain_entries(&mut self, keep: impl Fn(SiteEntry) -> bool) {
        let mut index = 0;
        self.forwards.retain(|_| {
            let k = keep(SiteEntry::Forward(index));
            index += 1;
            k
        });
        let mut index = 0;
        self.redirects.retain(|_| {
            let k = keep(SiteEntry::Redirect(index));
            index += 1;
            k
        });
        let mut index = 0;
        self.l4_routes.retain(|_| {
            let k = keep(SiteEntry::L4(index));
            index += 1;
            k
        });
    }
}

/// Keep, of the site ingresses making conflicting claims, those whose
/// attachment was created first, and report the rest.
///
/// The name breaks a tie, so the outcome is the same on every tick.
// spec: SITE#ingress-conflicts
pub(super) fn resolve_site_claims(mut data: SiteProxyData) -> (SiteProxyData, Vec<LostSiteClaim>) {
    let mut entries = data.entries();
    entries.sort_by_cached_key(|(entry, name, port, _)| {
        (
            data.created
                .get(&(name.clone(), *port))
                .cloned()
                .unwrap_or_default(),
            name.clone(),
            *entry,
        )
    });
    let candidates: Vec<Candidate<SiteIngressName>> = entries
        .iter()
        .map(|(_, name, _, claims)| Candidate {
            owner: name.clone(),
            claims: claims.clone(),
            rides_with: None,
        })
        .collect();
    let outcome = admit(&candidates);

    let mut lost = Vec::new();
    let mut dropped: BTreeSet<SiteEntry> = BTreeSet::new();
    for ((entry, name, _, _), admission) in entries.iter().zip(&outcome) {
        let Admission::Lost { mine, theirs, by } = admission else {
            continue;
        };
        dropped.insert(*entry);
        lost.push(LostSiteClaim {
            site_ingress: name.clone(),
            claim: mine.clone(),
            served: entries[*by].1.clone(),
            served_claim: theirs.clone(),
        });
    }
    data.retain_entries(|entry| !dropped.contains(&entry));
    (data, lost)
}

/// One side of an app's claim against a site ingress attachment's, which is
/// why both are left out of the proxy.
#[derive(Debug, Clone)]
pub(super) struct ConflictParty {
    pub app: AppName,
    pub ingress: String,
    /// The app's claim is its ingress's HTTP redirect, not the ingress itself.
    pub redirect: bool,
    pub app_claim: Claim,
    pub site_ingress: SiteIngressName,
    pub site_claim: Claim,
}

/// App ingresses and site ingress attachments making conflicting claims, and
/// what the proxy is to leave out because of them. The reconciler files faults
/// for each party and clears them on the first tick where the conflict no
/// longer appears.
// spec: SITE#ingress-conflicts
#[derive(Debug, Default, Clone)]
pub(super) struct ConflictReport {
    /// App claims against site attachment claims.
    pub parties: Vec<ConflictParty>,
    /// App ingresses left out because another app's ingress makes a
    /// conflicting claim.
    // spec: ING#claims-across-apps
    pub app_claims: Vec<LostAppClaim>,
    /// Site ingress attachments left out because an attachment of another site
    /// ingress, created first, makes a conflicting claim.
    // spec: SITE#ingress-conflicts
    pub site_claims: Vec<LostSiteClaim>,
    /// App ingresses left out whole because of a site attachment.
    dropped_ingresses: BTreeSet<(AppName, String)>,
    /// App ingresses whose redirect alone is left out because of a site
    /// attachment; the ingress itself is served.
    // spec: PRX#shared-ports
    dropped_redirects: BTreeSet<(AppName, String)>,
    dropped_site: BTreeSet<SiteEntry>,
}

/// Find every app claim and site attachment claim that conflict. Caller is
/// responsible for passing the result to [`drop_conflicting`], which removes
/// both sides before the proxy configuration is built.
// spec: SITE#ingress-conflicts
pub(super) fn detect_conflicts(apps: &[AppIngress], site: &SiteProxyData) -> ConflictReport {
    let site_entries = site.entries();
    let mut report = ConflictReport::default();

    let mut find = |app: &AppIngress, redirect: bool, claim: &Claim| {
        let mut found = false;
        for (entry, name, _, site_claims) in &site_entries {
            for site_claim in site_claims.iter().filter(|c| claim.conflicts_with(c)) {
                found = true;
                report.dropped_site.insert(*entry);
                report.parties.push(ConflictParty {
                    app: app.app.clone(),
                    ingress: app.name(),
                    redirect,
                    app_claim: claim.clone(),
                    site_ingress: name.clone(),
                    site_claim: site_claim.clone(),
                });
            }
        }
        found
    };

    for app in apps {
        let mut whole = false;
        for claim in app.def.claims() {
            whole |= find(app, false, &claim);
        }
        if whole {
            report
                .dropped_ingresses
                .insert((app.app.clone(), app.name()));
        } else if let Some(claim) = app.def.redirect_claim()
            && find(app, true, &claim)
        {
            report
                .dropped_redirects
                .insert((app.app.clone(), app.name()));
        }
    }
    report
}

/// Leave out of the app ingresses and the site data what `report` says to:
/// both sides of each conflict between them. An ingress whose redirect alone
/// conflicts keeps being served without it.
// spec: SITE#ingress-conflicts
// spec: PRX#shared-ports
pub(super) fn drop_conflicting(
    apps: Vec<AppIngress>,
    mut site: SiteProxyData,
    report: &ConflictReport,
) -> (Vec<AppIngress>, SiteProxyData) {
    let apps = apps
        .into_iter()
        .filter(|i| {
            !report
                .dropped_ingresses
                .contains(&(i.app.clone(), i.name()))
        })
        .map(|mut i| {
            if report
                .dropped_redirects
                .contains(&(i.app.clone(), i.name()))
            {
                i.def.redirect = None;
            }
            i
        })
        .collect();
    site.retain_entries(|entry| !report.dropped_site.contains(&entry));
    (apps, site)
}

#[cfg(test)]
mod tests {
    use std::net::Ipv6Addr;

    use super::*;
    use crate::defs::ingress::{HttpTermination, IngressDef};

    fn def(hostname: &str, port: u16) -> IngressDef {
        IngressDef {
            hostname: hostname.to_owned(),
            port: Port::new(i64::from(port)).unwrap(),
            tls: true,
            dtls: false,
            http_terminate: Some(HttpTermination::Http1),
            redirect: None,
            description: None,
        }
    }

    fn upstream() -> ServiceUpstream {
        ServiceUpstream {
            routes: Vec::new(),
            redirects: Vec::new(),
            service_ip: Ipv6Addr::from([0xfd, 0x5e, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
            service_port: 8080,
            proxy: crate::system::types::RouteProxy::unlimited(
                crate::defs::service::ResolvedRouteProxy::default(),
            ),
        }
    }

    fn redirect_target() -> RedirectTarget {
        RedirectTarget {
            url: "https://new.example.com".to_owned(),
            code: 307,
            preserve_path: true,
        }
    }

    fn site(name: &str) -> SiteIngressName {
        SiteIngressName::new(name).unwrap()
    }

    fn snapshot(script: &str) -> AppSnapshot {
        let name = AppName::new("demo").unwrap();
        let (app, err) = crate::runtime::apps::evaluate_script(
            &name,
            script,
            &std::collections::BTreeMap::new(),
            &crate::ScriptLimits::default(),
        );
        assert!(err.is_none(), "test script must evaluate: {err:?}");
        AppSnapshot {
            name,
            desired: Default::default(),
            steady: true,
            app_def: (*app.def.load_full()).clone(),
            phase: crate::runtime::AppPhase::Installed,
            phase_handle: std::sync::Arc::new(parking_lot::Mutex::new(
                crate::runtime::AppPhase::Installed,
            )),
            warm_cert_hostnames: Default::default(),
            app_priority: Default::default(),
            install_seq: Some(1),
        }
    }

    // spec: SITE#attachments
    #[test]
    fn the_forwarded_port_is_found_by_the_attachments_transport() {
        let app = snapshot(
            r#"
            let dns = app.service("dns");
            let web = app.service("web").http(80);
            app.job("aaa").image("docker.io/library/busybox:1").tcp(1, dns.port(5999));
            app.deployment("ccc").image("docker.io/library/nginx:1").udp(2, dns.port(5353));
            app.deployment("bbb").image("docker.io/library/nginx:1")
                .http(8080, web.route("/"))
                .tcp(3, dns.port(5300));
        "#,
        );
        let port = |service, protocol| scan_apps_for_service_port(&app, service, protocol);

        // Deployments before Jobs, and a UDP attachment takes a UDP binding.
        assert_eq!(port("dns", AttachmentProtocol::Tcp), Some(5300));
        assert_eq!(port("dns", AttachmentProtocol::Udp), Some(5353));
        // An HTTP route binding serves a TCP transport, never UDP.
        assert_eq!(port("web", AttachmentProtocol::Http), Some(80));
        assert_eq!(port("web", AttachmentProtocol::Udp), None);
    }

    // spec: PRX#route-emission
    // spec: SITE#attachments
    #[test]
    fn an_http_attachments_fallback_route_reaches_where_an_app_ingress_would() {
        let app = snapshot(
            r#"
            let web = app.service("web");
            web.http(8080);
            app.deployment("app").image("docker.io/library/nginx:1").tcp(1, web.port(7000));
        "#,
        );
        let attachment = |port, protocol| SiteIngressAttachment {
            site_ingress: site("edge"),
            port,
            protocol,
            target: AttachmentTarget::Forward {
                app: app.name.clone(),
                service: seedling_protocol::names::AppServiceName::new("web").unwrap(),
            },
            created_at: String::new(),
        };
        let upstream_port = |att: SiteIngressAttachment| {
            let AttachmentTarget::Forward {
                app: target,
                service,
            } = &att.target
            else {
                unreachable!()
            };
            resolve_forward_upstream(
                target,
                service,
                &att,
                std::slice::from_ref(&app),
                &Default::default(),
                &"fd5e:ed12:3456::/48".parse().unwrap(),
                &crate::runtime::EphemeralInstanceRegistry::new(),
            )
            .expect("resolves")
            .service_port
        };

        assert_eq!(
            upstream_port(attachment(443, AttachmentProtocol::Http)),
            8080
        );
        assert_eq!(
            upstream_port(attachment(4433, AttachmentProtocol::Tcp)),
            7000
        );
    }

    fn app_ingress(app: &AppName, def: IngressDef) -> AppIngress {
        let serve = if def.dtls {
            AppServe::Dtls(DtlsRoute {
                port: def.port.get(),
                hostname: def.hostname.clone(),
                upstream: "[fd5e::1]:5684".to_owned(),
                ingress: ingress_resource_name(&def),
            })
        } else if def.http_terminate.is_none() {
            AppServe::Tcp(L4Route {
                port: def.port.get(),
                proto: L4Proto::Tcp,
                upstreams: vec!["[fd5e::1]:5432".to_owned()],
                terminate_tls: def.tls,
                hostname: def.tls.then(|| def.hostname.clone()),
            })
        } else {
            AppServe::Http(Box::new(upstream()))
        };
        AppIngress {
            app: app.clone(),
            def,
            serve,
        }
    }

    fn tls_def(hostname: &str, port: u16) -> IngressDef {
        IngressDef {
            http_terminate: None,
            ..def(hostname, port)
        }
    }

    fn dtls_def(hostname: &str, port: u16) -> IngressDef {
        IngressDef {
            tls: false,
            dtls: true,
            http_terminate: None,
            ..def(hostname, port)
        }
    }

    fn pass_through_def(hostname: &str, port: u16) -> IngressDef {
        IngressDef {
            tls: false,
            http_terminate: None,
            ..def(hostname, port)
        }
    }

    fn plaintext(hostname: &str, port: u16) -> IngressDef {
        IngressDef {
            tls: false,
            ..def(hostname, port)
        }
    }

    // spec: SITE#ingress-conflicts
    #[test]
    fn an_app_ingress_conflicts_with_a_redirect_attachment_on_its_hostname_and_port() {
        let app = AppName::new("web").unwrap();
        let apps = vec![app_ingress(&app, def("shared.example.com", 443))];
        let data = SiteProxyData {
            redirects: vec![(
                site("legacy"),
                def("shared.example.com", 443),
                redirect_target(),
            )],
            ..site_data(vec![], vec![])
        };
        let report = detect_conflicts(&apps, &data);
        assert_eq!(report.parties.len(), 2, "its TCP and its UDP claim");
        let (kept_apps, kept_site) = drop_conflicting(apps, data, &report);
        assert!(kept_apps.is_empty() && kept_site.redirects.is_empty());
    }

    fn l4_entry(name: &str, port: u16, proto: L4Proto) -> SiteL4Entry {
        SiteL4Entry {
            site_ingress: site(name),
            route: L4Route {
                port,
                proto,
                upstreams: vec!["[fd5e::1]:5432".to_owned()],
                terminate_tls: false,
                hostname: None,
            },
        }
    }

    fn site_data(forwards: Vec<(&str, IngressDef)>, l4: Vec<SiteL4Entry>) -> SiteProxyData {
        SiteProxyData {
            forwards: forwards
                .into_iter()
                .map(|(name, def)| (site(name), def, upstream()))
                .collect(),
            redirects: vec![],
            l4_routes: l4,
            unresolved: vec![],
            created: Default::default(),
        }
    }

    fn first_install_first(app: &AppName) -> Option<i64> {
        match app.as_str() {
            "first" => Some(1),
            "second" => Some(2),
            _ => None,
        }
    }

    // spec: ING#claims-across-apps
    #[test]
    fn the_app_that_installed_first_keeps_a_shared_hostname_and_port() {
        let first = AppName::new("first").unwrap();
        let second = AppName::new("second").unwrap();
        let never = AppName::new("never").unwrap();
        let ingresses = vec![
            app_ingress(&never, def("shared.example.com", 443)),
            app_ingress(&second, def("shared.example.com", 443)),
            app_ingress(&first, def("shared.example.com", 443)),
            app_ingress(&second, def("shared.example.com", 8443)),
        ];

        let (kept, lost) = resolve_app_claims(ingresses, first_install_first);
        let kept: Vec<_> = kept
            .iter()
            .map(|i| (i.app.as_str(), i.def.port.get()))
            .collect();
        assert_eq!(kept, vec![("first", 443), ("second", 8443)]);
        let mut losers: Vec<_> = lost.iter().map(|l| l.app.as_str()).collect();
        losers.sort();
        assert_eq!(losers, vec!["never", "second"]);
        assert!(
            lost.iter()
                .all(|l| l.served.app == first && l.claim.port == 443)
        );

        // Once the served ingress stops claiming them, the next is served.
        let (kept, lost) = resolve_app_claims(
            vec![
                app_ingress(&never, def("shared.example.com", 443)),
                app_ingress(&second, def("shared.example.com", 443)),
            ],
            first_install_first,
        );
        assert_eq!(kept[0].app, second);
        assert_eq!(lost[0].app, never);
    }

    // spec: PRX#shared-ports
    // spec: ING#claims-across-apps
    #[test]
    fn apps_share_a_port_by_hostname_where_their_claims_are_of_one_kind() {
        let first = AppName::new("first").unwrap();
        let second = AppName::new("second").unwrap();
        let (kept, lost) = resolve_app_claims(
            vec![
                app_ingress(&first, tls_def("a.example.com", 443)),
                app_ingress(&second, def("b.example.com", 443)),
            ],
            first_install_first,
        );
        assert_eq!(kept.len(), 2, "TLS beside HTTPS on one port");
        assert!(lost.is_empty());
    }

    // spec: PRX#shared-ports
    // spec: ING#claims-across-apps
    #[test]
    fn pass_through_takes_its_whole_port_from_a_later_apps_ingress() {
        let first = AppName::new("first").unwrap();
        let second = AppName::new("second").unwrap();
        let (kept, lost) = resolve_app_claims(
            vec![
                app_ingress(&second, def("b.example.com", 443)),
                app_ingress(&first, pass_through_def("a.example.com", 443)),
            ],
            first_install_first,
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].app, first);
        assert_eq!(lost[0].app, second);
        assert_eq!(
            lost[0].claim.to_string(),
            "tcp/443 (TLS b.example.com)",
            "names the port, the transport and the hostname"
        );
    }

    // spec: PRX#shared-ports
    #[test]
    fn an_ingress_left_out_is_left_out_whole_so_dtls_beside_http3_loses_its_tcp_claim_too() {
        let first = AppName::new("first").unwrap();
        let second = AppName::new("second").unwrap();
        let (kept, lost) = resolve_app_claims(
            vec![
                app_ingress(&first, dtls_def("voip.example.com", 443)),
                app_ingress(&second, def("web.example.com", 443)),
                // The HTTPS ingress's TCP claim must not keep this one out once
                // it is itself left out.
                app_ingress(&second, tls_def("db.example.com", 443)),
            ],
            first_install_first,
        );
        let kept: Vec<_> = kept.iter().map(|i| i.def.hostname.as_str()).collect();
        assert_eq!(kept, ["voip.example.com", "db.example.com"]);
        assert_eq!(lost.len(), 1);
        assert_eq!(
            lost[0].claim.to_string(),
            "udp/443 (HTTP/3 web.example.com)"
        );
    }

    // spec: PRX#shared-ports
    #[test]
    fn a_redirect_that_loses_is_left_out_alone() {
        let first = AppName::new("first").unwrap();
        let second = AppName::new("second").unwrap();
        let mut redirecting = def("b.example.com", 443);
        redirecting.redirect = Some(crate::defs::ingress::RedirectDef {
            port: Port::new(80).unwrap(),
            code: 307,
        });
        let (kept, lost) = resolve_app_claims(
            vec![
                app_ingress(&first, pass_through_def("a.example.com", 80)),
                app_ingress(&second, redirecting),
            ],
            first_install_first,
        );
        assert_eq!(kept.len(), 2);
        assert!(kept[1].def.redirect.is_none());
        assert!(lost[0].redirect);
    }

    #[test]
    fn no_conflict_when_disjoint() {
        let app = AppName::new("web").unwrap();
        let apps = vec![app_ingress(&app, def("a.example.com", 443))];
        let data = site_data(vec![("front", def("b.example.com", 8443))], vec![]);
        let report = detect_conflicts(&apps, &data);
        assert!(report.parties.is_empty());
    }

    // spec: SITE#ingress-conflicts
    #[test]
    fn an_app_ingress_and_an_attachment_for_one_hostname_and_port_are_both_left_out() {
        let app = AppName::new("web").unwrap();
        let apps = vec![app_ingress(&app, def("shared.example.com", 443))];
        let data = site_data(
            vec![
                ("front", def("shared.example.com", 443)),
                ("solo", def("solo.example.com", 8443)),
            ],
            vec![],
        );
        let report = detect_conflicts(&apps, &data);
        assert_eq!(report.parties.len(), 2, "its TCP and its UDP claim");
        assert_eq!(report.parties[0].site_ingress, site("front"));

        let (kept_apps, kept_site) = drop_conflicting(apps, data, &report);
        assert!(kept_apps.is_empty());
        assert_eq!(kept_site.forwards.len(), 1);
        assert_eq!(kept_site.forwards[0].0, site("solo"));
    }

    // spec: SITE#ingress-conflicts
    // spec: PRX#shared-ports
    #[test]
    fn app_and_attachment_serving_one_port_for_different_hostnames_do_not_conflict() {
        let app = AppName::new("web").unwrap();
        let apps = vec![app_ingress(&app, tls_def("a.example.com", 443))];
        let data = site_data(vec![("front", def("b.example.com", 443))], vec![]);
        assert!(detect_conflicts(&apps, &data).parties.is_empty());
    }

    // spec: SITE#ingress-conflicts
    #[test]
    fn a_pass_through_app_ingress_conflicts_with_a_tcp_attachment_on_its_port() {
        let app = AppName::new("web").unwrap();
        let apps = vec![app_ingress(&app, pass_through_def("a.example.com", 5432))];
        let data = site_data(vec![], vec![l4_entry("postgres", 5432, L4Proto::Tcp)]);
        let report = detect_conflicts(&apps, &data);
        assert_eq!(report.parties.len(), 1);
        assert_eq!(report.parties[0].site_ingress, site("postgres"));

        let (kept_apps, kept_site) = drop_conflicting(apps, data, &report);
        assert!(kept_apps.is_empty() && kept_site.l4_routes.is_empty());
    }

    // spec: SITE#ingress-conflicts
    #[test]
    fn a_dtls_app_ingress_conflicts_with_a_udp_attachment_but_not_a_tcp_one() {
        let app = AppName::new("web").unwrap();
        let apps = vec![app_ingress(&app, dtls_def("voip.example.com", 5684))];
        let data = site_data(vec![], vec![l4_entry("tcp", 5684, L4Proto::Tcp)]);
        assert!(detect_conflicts(&apps, &data).parties.is_empty());
        let data = site_data(vec![], vec![l4_entry("udp", 5684, L4Proto::Udp)]);
        assert_eq!(detect_conflicts(&apps, &data).parties.len(), 1);
    }

    // spec: SITE#ingress-conflicts
    #[test]
    fn dtls_conflicts_with_a_site_https_attachments_http3_claim() {
        let app = AppName::new("web").unwrap();
        let apps = vec![app_ingress(&app, dtls_def("voip.example.com", 443))];
        let data = site_data(vec![("front", def("web.example.com", 443))], vec![]);
        let report = detect_conflicts(&apps, &data);
        assert_eq!(report.parties.len(), 1);
        assert_eq!(
            report.parties[0].site_claim.to_string(),
            "udp/443 (HTTP/3 web.example.com)"
        );
    }

    fn created(
        entries: &[(&str, u16, &str)],
    ) -> std::collections::BTreeMap<(SiteIngressName, u16), String> {
        entries
            .iter()
            .map(|(name, port, at)| ((site(name), *port), (*at).to_owned()))
            .collect()
    }

    // spec: SITE#ingress-conflicts
    #[test]
    fn the_site_ingress_whose_attachment_was_created_first_is_served() {
        let data = SiteProxyData {
            forwards: vec![
                (site("second"), def("shared.example.com", 443), upstream()),
                (site("first"), def("shared.example.com", 443), upstream()),
                (site("second"), def("shared.example.com", 8443), upstream()),
            ],
            redirects: vec![],
            l4_routes: vec![],
            unresolved: vec![],
            created: created(&[
                ("first", 443, "2026-01-01T00:00:00Z"),
                ("second", 443, "2026-02-01T00:00:00Z"),
                ("second", 8443, "2026-02-01T00:00:00Z"),
            ]),
        };

        let (kept, lost) = resolve_site_claims(data);
        let served: Vec<_> = kept
            .forwards
            .iter()
            .map(|(n, d, _)| (n.as_str().to_owned(), d.port.get()))
            .collect();
        assert_eq!(
            served,
            vec![("first".to_owned(), 443), ("second".to_owned(), 8443)],
            "another port on the same hostname is unaffected"
        );
        assert_eq!(lost.len(), 2, "its TCP claim and its UDP claim");
        assert!(lost.iter().all(|l| l.site_ingress == site("second")));
        assert!(lost.iter().all(|l| l.served == site("first")));
        assert!(lost.iter().all(|l| l.claim.port == 443));

        // Once the served attachment no longer claims the pair, the other is
        // served.
        let data = SiteProxyData {
            forwards: vec![(site("second"), def("shared.example.com", 443), upstream())],
            redirects: vec![],
            l4_routes: vec![],
            unresolved: vec![],
            created: created(&[("second", 443, "2026-02-01T00:00:00Z")]),
        };
        let (kept, lost) = resolve_site_claims(data);
        assert_eq!(kept.forwards.len(), 1);
        assert!(lost.is_empty());
    }

    // spec: SITE#ingress-conflicts
    // spec: PRX#shared-ports
    #[test]
    fn site_attachments_for_different_hostnames_share_a_port() {
        let data = site_data(
            vec![
                ("one", def("one.example.com", 443)),
                ("two", def("two.example.com", 443)),
            ],
            vec![],
        );
        let (kept, lost) = resolve_site_claims(data);
        assert_eq!(kept.forwards.len(), 2);
        assert!(lost.is_empty());
    }

    // spec: PRX#shared-ports
    #[test]
    fn an_app_ingress_redirect_conflicts_with_a_plaintext_attachment_on_its_port() {
        let app = AppName::new("web").unwrap();
        let mut ingress = def("clinic.example.com", 443);
        ingress.redirect = Some(crate::defs::ingress::RedirectDef {
            port: Port::new(80).unwrap(),
            code: 307,
        });
        let apps = vec![app_ingress(&app, ingress)];
        let data = site_data(
            vec![
                ("plain", plaintext("clinic.example.com", 80)),
                // A TLS attachment for another hostname on the redirect's
                // port is not a plaintext ingress, but it is a claim of
                // another kind on the same TCP port.
                ("tls", def("other.example.com", 80)),
                // Plaintext for another hostname shares the port.
                ("other", plaintext("other.example.com", 80)),
            ],
            vec![],
        );

        let report = detect_conflicts(&apps, &data);
        let sites: BTreeSet<_> = report
            .parties
            .iter()
            .map(|p| p.site_ingress.as_str().to_owned())
            .collect();
        assert_eq!(
            sites,
            BTreeSet::from(["plain".to_owned(), "tls".to_owned()])
        );
        assert!(report.parties.iter().all(|p| p.redirect));

        // The ingress keeps serving; only its redirect and the attachments go.
        let (kept_apps, kept_site) = drop_conflicting(apps, data, &report);
        assert_eq!(kept_apps.len(), 1);
        assert!(kept_apps[0].def.redirect.is_none());
        let kept: Vec<_> = kept_site
            .forwards
            .iter()
            .map(|(n, _, _)| n.as_str())
            .collect();
        assert_eq!(kept, ["other"]);
    }
}
