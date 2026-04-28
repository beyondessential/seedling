use std::collections::BTreeSet;

use ipnet::Ipv6Net;
use seedling_protocol::names::{AppName, SiteIngressName};

use super::AppSnapshot;
use crate::{
    defs::{
        Port,
        ingress::{HttpTermination, IngressDef},
        resource::{Resource, ResourceKind},
    },
    runtime::{
        InstanceRegistry,
        site_ingress_attachments::{
            self, AttachmentProtocol, AttachmentTarget, SiteIngressAttachment,
        },
        site_ingresses::{self, SiteIngressDef, TlsProvider},
    },
    system::translate::proxy::{RedirectTarget, ServiceUpstream, instance_ipv6},
};

/// Result of resolving every site-ingress attachment against the current
/// app set + registry. Entries that couldn't be resolved (e.g. their target
/// app/service doesn't exist) appear in `missing_targets`; the reconciler
/// files faults from that set so operators see a clear error.
pub(super) struct SiteProxyData {
    pub forwards: Vec<(SiteIngressName, IngressDef, ServiceUpstream)>,
    pub redirects: Vec<(SiteIngressName, IngressDef, RedirectTarget)>,
    /// `(site_ingress, port, protocol, reason)` for entries that couldn't
    /// be resolved this tick. Used for fault filing.
    pub unresolved: Vec<UnresolvedAttachment>,
}

#[derive(Debug, Clone)]
pub(super) struct UnresolvedAttachment {
    pub site_ingress: String,
    pub port: u16,
    pub protocol: AttachmentProtocol,
    pub reason: String,
}

/// Snapshot of the site ingress / attachment tables, loaded once per
/// reconcile tick. The collect step is a pure function over this snapshot
/// so it can run alongside the other phase computations without holding
/// the DB lock.
pub(super) struct SiteIngressSnapshot {
    pub ingresses: Vec<SiteIngressDef>,
    pub attachments: Vec<SiteIngressAttachment>,
}

// r[impl ingress.site] r[impl ingress.site.attachment]
pub(super) fn load(db: &crate::runtime::db::Db) -> SiteIngressSnapshot {
    let ingresses = site_ingresses::list(db).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "site_proxy: failed to list site ingresses; using empty");
        Vec::new()
    });
    let attachments = site_ingress_attachments::list_all(db).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "site_proxy: failed to list site ingress attachments; using empty");
        Vec::new()
    });
    SiteIngressSnapshot {
        ingresses,
        attachments,
    }
}

// r[impl ingress.site] r[impl ingress.site.attachment]
pub(super) fn collect(
    snapshot: &SiteIngressSnapshot,
    apps: &[AppSnapshot],
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
) -> SiteProxyData {
    let mut data = SiteProxyData {
        forwards: Vec::new(),
        redirects: Vec::new(),
        unresolved: Vec::new(),
    };

    for ingress in &snapshot.ingresses {
        // r[impl ingress.site.lifecycle] discovered ingresses that have lost
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
            resolve_attachment(ingress, att, apps, node_prefix, registry, &mut data);
        }
    }

    data
}

fn resolve_attachment(
    ingress: &SiteIngressDef,
    att: &SiteIngressAttachment,
    apps: &[AppSnapshot],
    node_prefix: &Ipv6Net,
    registry: &dyn InstanceRegistry,
    data: &mut SiteProxyData,
) {
    // L4 (TCP/UDP) attachments need the L4 proxy/nftables path; deferred to
    // a follow-up. The OI handler currently allows the protocol values, so
    // we skip them here and surface the limitation as an unresolved entry.
    let term = match att.protocol {
        AttachmentProtocol::Http => HttpTermination::Http1,
        AttachmentProtocol::Http2 => HttpTermination::Http2,
        AttachmentProtocol::Tcp | AttachmentProtocol::Udp => {
            data.unresolved.push(UnresolvedAttachment {
                site_ingress: ingress.name.as_str().to_owned(),
                port: att.port,
                protocol: att.protocol,
                reason: format!(
                    "L4 ({}) attachments are not yet supported on site ingresses",
                    att.protocol
                ),
            });
            return;
        }
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
    };

    match &att.target {
        AttachmentTarget::Forward { app, service } => {
            match resolve_forward_upstream(app, service, apps, node_prefix, registry) {
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
    apps: &[AppSnapshot],
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
    let service_ip = instance_ipv6(node_prefix, &svc_instance);

    // Find the listening pod port for this service (matches
    // `super::proxy::find_upstream_port`'s strategy). When no binding is
    // declared we fall back to a sentinel so the operator sees an obvious
    // failure rather than a silent 502.
    let upstream_port = scan_apps_for_service_port(snapshot, svc_name_str)
        .ok_or_else(|| format!("no pod binding found for {target_app}/{svc_name_str}"))?;

    Ok(ServiceUpstream {
        service_ip,
        service_port: upstream_port,
    })
}

fn scan_apps_for_service_port(snapshot: &AppSnapshot, service_name: &str) -> Option<u16> {
    for resource in snapshot.app_def.resources.values() {
        let result = match resource {
            Resource::Deployment(dep) => {
                let def = dep.def.lock();
                let pod = def.pod.lock();
                pod_first_port(&pod, service_name)
            }
            Resource::Job(job) => {
                let def = job.def.lock();
                let pod = def.pod.lock();
                pod_first_port(&pod, service_name)
            }
            _ => None,
        };
        if let Some(p) = result {
            return Some(p);
        }
    }
    None
}

fn pod_first_port(pod: &crate::defs::pod::PodDef, service_name: &str) -> Option<u16> {
    for b in &pod.tcp_bindings {
        if b.service_port.service.name().as_str() == service_name {
            return Some(b.service_port.port.get());
        }
    }
    for b in &pod.http_bindings {
        if b.route.http.service.name().as_str() == service_name {
            return Some(b.route.http.port.get());
        }
    }
    None
}

/// (hostname, port) tuples that the proxy config rejected because both an
/// app ingress and a site-ingress attachment claimed them. Both sides of
/// each conflict are dropped from the proxy config; the reconciler files
/// faults for each party and clears them on the first tick where the
/// conflict no longer appears.
// r[impl ingress.site.conflict]
#[derive(Debug, Default, Clone)]
pub(super) struct ConflictReport {
    /// Conflict tuples observed this tick.
    pub conflicts: BTreeSet<(String, u16)>,
    /// Per-conflict roster of involved parties, used for fault descriptions.
    pub parties: Vec<ConflictParties>,
}

#[derive(Debug, Clone)]
pub(super) struct ConflictParties {
    pub hostname: String,
    pub port: u16,
    /// Apps with a colliding ingress on this `(hostname, port)`.
    pub apps: Vec<(AppName, String /* ingress resource name */)>,
    /// Site ingresses with a colliding attachment on this `(hostname, port)`.
    pub site: Vec<String /* site ingress name */>,
}

/// Find every `(hostname, port)` tuple claimed by both an app ingress and a
/// site-ingress attachment. Caller is responsible for removing the matching
/// entries from `app_pairs`, `site_data.forwards`, and `site_data.redirects`
/// before passing them to `build_proxy_config`.
// r[impl ingress.site.conflict]
pub(super) fn detect_conflicts(
    app_pairs: &[(AppName, IngressDef, ServiceUpstream)],
    site_data: &SiteProxyData,
) -> ConflictReport {
    use std::collections::BTreeMap;

    let mut app_index: BTreeMap<(String, u16), Vec<(AppName, String)>> = BTreeMap::new();
    for (app, def, _up) in app_pairs {
        app_index
            .entry((def.hostname.clone(), def.port.get()))
            .or_default()
            .push((app.clone(), ingress_resource_name(def)));
    }

    let mut site_index: BTreeMap<(String, u16), Vec<String>> = BTreeMap::new();
    for (name, def, _up) in &site_data.forwards {
        site_index
            .entry((def.hostname.clone(), def.port.get()))
            .or_default()
            .push(name.as_str().to_owned());
    }
    for (name, def, _r) in &site_data.redirects {
        site_index
            .entry((def.hostname.clone(), def.port.get()))
            .or_default()
            .push(name.as_str().to_owned());
    }

    let mut report = ConflictReport::default();
    for (key, apps) in &app_index {
        if let Some(site) = site_index.get(key) {
            report.conflicts.insert(key.clone());
            report.parties.push(ConflictParties {
                hostname: key.0.clone(),
                port: key.1,
                apps: apps.clone(),
                site: site.clone(),
            });
        }
    }
    report
}

/// Reconstruct the resource name an ingress occupies inside an `AppDef`'s
/// resource map: `<hostname>:<port>`. Mirrors
/// `crate::defs::ingress::ingress_resource_name` without depending on the
/// definitions side of the codebase here.
fn ingress_resource_name(def: &IngressDef) -> String {
    format!("{}:{}", def.hostname, def.port.get())
}

/// Helper to drop entries whose `(hostname, port)` is in `conflicts`.
pub(super) fn drop_conflicting_app_pairs(
    pairs: Vec<(AppName, IngressDef, ServiceUpstream)>,
    conflicts: &BTreeSet<(String, u16)>,
) -> Vec<(AppName, IngressDef, ServiceUpstream)> {
    pairs
        .into_iter()
        .filter(|(_app, def, _u)| !conflicts.contains(&(def.hostname.clone(), def.port.get())))
        .collect()
}

pub(super) fn drop_conflicting_site_data(
    mut data: SiteProxyData,
    conflicts: &BTreeSet<(String, u16)>,
) -> SiteProxyData {
    data.forwards
        .retain(|(_n, def, _u)| !conflicts.contains(&(def.hostname.clone(), def.port.get())));
    data.redirects
        .retain(|(_n, def, _r)| !conflicts.contains(&(def.hostname.clone(), def.port.get())));
    data
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
        }
    }

    fn upstream() -> ServiceUpstream {
        ServiceUpstream {
            service_ip: Ipv6Addr::from([0xfd, 0x5e, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
            service_port: 8080,
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

    #[test]
    fn no_conflict_when_disjoint() {
        let app = AppName::new("web").unwrap();
        let app_pairs = vec![(app.clone(), def("a.example.com", 443), upstream())];
        let data = SiteProxyData {
            forwards: vec![(site("front"), def("b.example.com", 443), upstream())],
            redirects: vec![],
            unresolved: vec![],
        };
        let report = detect_conflicts(&app_pairs, &data);
        assert!(report.conflicts.is_empty());
    }

    #[test]
    fn conflict_on_same_host_and_port() {
        let app = AppName::new("web").unwrap();
        let app_pairs = vec![(app.clone(), def("shared.example.com", 443), upstream())];
        let data = SiteProxyData {
            forwards: vec![(site("front"), def("shared.example.com", 443), upstream())],
            redirects: vec![],
            unresolved: vec![],
        };
        let report = detect_conflicts(&app_pairs, &data);
        assert_eq!(report.conflicts.len(), 1);
        assert!(
            report
                .conflicts
                .contains(&("shared.example.com".to_owned(), 443))
        );
        assert_eq!(report.parties.len(), 1);
        assert_eq!(report.parties[0].apps.len(), 1);
        assert_eq!(report.parties[0].site.len(), 1);
        assert_eq!(report.parties[0].site[0], "front");
    }

    #[test]
    fn conflict_with_redirect_side() {
        let app = AppName::new("web").unwrap();
        let app_pairs = vec![(app.clone(), def("shared.example.com", 443), upstream())];
        let data = SiteProxyData {
            forwards: vec![],
            redirects: vec![(
                site("legacy"),
                def("shared.example.com", 443),
                redirect_target(),
            )],
            unresolved: vec![],
        };
        let report = detect_conflicts(&app_pairs, &data);
        assert_eq!(report.conflicts.len(), 1);
    }

    #[test]
    fn different_ports_on_same_host_do_not_conflict() {
        let app = AppName::new("web").unwrap();
        let app_pairs = vec![(app.clone(), def("shared.example.com", 443), upstream())];
        let data = SiteProxyData {
            forwards: vec![(site("front"), def("shared.example.com", 8080), upstream())],
            redirects: vec![],
            unresolved: vec![],
        };
        let report = detect_conflicts(&app_pairs, &data);
        assert!(report.conflicts.is_empty());
    }

    #[test]
    fn drop_filters_both_sides() {
        let app = AppName::new("web").unwrap();
        let app_pairs = vec![
            (app.clone(), def("kept.example.com", 443), upstream()),
            (app.clone(), def("dropped.example.com", 443), upstream()),
        ];
        let data = SiteProxyData {
            forwards: vec![
                (site("front"), def("dropped.example.com", 443), upstream()),
                (site("solo"), def("solo.example.com", 443), upstream()),
            ],
            redirects: vec![],
            unresolved: vec![],
        };
        let conflicts: BTreeSet<(String, u16)> = [("dropped.example.com".to_owned(), 443)]
            .into_iter()
            .collect();
        let kept_app = drop_conflicting_app_pairs(app_pairs, &conflicts);
        assert_eq!(kept_app.len(), 1);
        assert_eq!(kept_app[0].1.hostname, "kept.example.com");
        let kept_site = drop_conflicting_site_data(data, &conflicts);
        assert_eq!(kept_site.forwards.len(), 1);
        assert_eq!(kept_site.forwards[0].1.hostname, "solo.example.com");
    }
}
