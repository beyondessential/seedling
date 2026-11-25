use std::sync::Arc;

use rhai::{CustomType, EvalAltResult, TypeBuilder};

use crate::runtime::barrier::runtime::is_in_action_closure;

use super::{
    Freezable, Holder, Port,
    enums::{Output, Terminate},
    resource::ResourceName,
    service::{Service, redirect_code},
};

#[derive(Debug, Clone)]
pub struct IngressDef {
    pub hostname: String,
    pub port: Port,
    pub tls: bool,
    pub dtls: bool,
    pub http_terminate: Option<HttpTermination>,
    pub redirect: Option<RedirectDef>,
    // spec: BSL#descriptions
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum HttpTermination {
    Http1,
    Http2,
}

#[derive(Debug, Clone)]
pub struct RedirectDef {
    pub port: Port,
    pub code: u16,
}

/// Whether `hostname` meets the rules for an ingress hostname, or why not.
// spec: ING#hostnames
pub(crate) fn check_hostname(hostname: &str) -> Result<(), String> {
    if hostname.is_empty() || hostname.len() > 253 {
        return Err(format!(
            "hostname must be 1–253 characters, got {}",
            hostname.len()
        ));
    }

    if hostname.contains('*') {
        return Err("wildcard hostnames are not permitted".to_owned());
    }

    for label in hostname.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(format!(
                "each hostname label must be 1–63 characters, got '{}' ({})",
                label,
                label.len()
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(format!(
                "hostname label must not start or end with a hyphen: '{label}'"
            ));
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(format!(
                "hostname label contains invalid characters: '{label}'"
            ));
        }
    }

    Ok(())
}

// spec: ING#declaration
#[derive(Debug, Clone)]
pub struct Ingress {
    pub name: ResourceName,
    pub service: Service,
    pub def: Holder<IngressDef>,
    pub frozen: bool,
}

impl super::Freezable for Ingress {
    // spec: APP#static-resource-immutability
    fn is_frozen(&self) -> bool {
        self.frozen || is_in_action_closure()
    }
}

impl Ingress {
    pub fn new(service: Service, hostname: String, port: Port) -> Self {
        // One form for a hostname, from the moment it is declared. A DNS name
        // is case-insensitive, a script may write one in any case, and what a
        // client sends as SNI is matched against this exactly: declared in
        // another case, the hostname would be one no handshake ever names, and
        // every decision made for it would be made for a name nothing asks
        // about.
        // spec: ING#hostnames
        let hostname = hostname.to_ascii_lowercase();
        // The ingress's resource name is its (hostname, port) tuple,
        // not the underlying service's name: a single service is
        // allowed multiple ingresses (different hostnames, redirect
        // settings, ports), and they need distinct identities so the
        // app's resource map doesn't collapse them.
        // spec: ING#declaration
        let name: ResourceName = Arc::new(ingress_resource_name(&hostname, port));
        Self {
            name,
            service: service.clone(),
            def: Holder::new(
                IngressDef {
                    hostname,
                    port,
                    tls: false,
                    dtls: false,
                    http_terminate: None,
                    redirect: None,
                    description: None,
                }
                .into(),
            ),
            frozen: false,
        }
    }
}

/// Construct the resource name we use to key ingresses inside an
/// `AppDef`. Format is `"<hostname>:<port>"` — readable, stable, and
/// unique per (hostname, port) tuple. Exposed so the conflict check
/// in `service::ingress()` can build the same key without duplicating
/// the format.
pub fn ingress_resource_name(hostname: &str, port: Port) -> String {
    format!("{hostname}:{}", port.get())
}

/// Refuse a definition in which an ingress declares a redirect and its
/// final termination is not HTTPS. Checked once the definition has evaluated,
/// so the order of the `tls()` and `redirect` calls does not matter.
// spec: ING#redirect-to-https
pub(crate) fn check_redirect_terminations(def: &super::app::AppDef) -> Result<(), String> {
    for resource in def.resources.values() {
        let super::resource::Resource::Ingress(ingress) = resource else {
            continue;
        };
        let ingress_def = ingress.def.lock();
        if ingress_def.redirect.is_some() && ingress_def.http_terminate.is_none() {
            return Err(format!(
                "ingress '{}' declares redirect(), which requires an HTTPS termination, \
                 e.g. tls(Terminate.Https, Output.Http1)",
                ingress.name
            ));
        }
    }
    Ok(())
}

/// The transport a [`Claim`] is made on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClaimTransport {
    Tcp,
    Udp,
}

impl std::fmt::Display for ClaimTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        })
    }
}

/// What a claim on a hostname is a claim of. Claims of one kind for different
/// hostnames share a port, since the proxy tells their traffic apart by the
/// name the client sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClaimKind {
    /// TLS on TCP, which includes HTTPS; told apart by SNI.
    Tls,
    /// HTTP/3 on UDP; told apart by SNI.
    Http3,
    /// Plaintext HTTP on TCP; told apart by the request's `Host`.
    PlaintextHttp,
}

impl std::fmt::Display for ClaimKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Tls => "TLS",
            Self::Http3 => "HTTP/3",
            Self::PlaintextHttp => "plaintext HTTP",
        })
    }
}

/// How much of a port a claim takes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClaimScope {
    /// Traffic passed through or terminated as DTLS carries nothing the proxy
    /// reads a hostname from, so nothing else can share the port.
    WholePort,
    /// The port for one hostname, among claims of the same kind.
    Hostname { kind: ClaimKind, hostname: String },
}

/// One transport an ingress serves on its port.
// spec: PRX#shared-ports
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Claim {
    pub port: u16,
    pub transport: ClaimTransport,
    pub scope: ClaimScope,
}

impl Claim {
    pub fn whole_port(port: u16, transport: ClaimTransport) -> Self {
        Self {
            port,
            transport,
            scope: ClaimScope::WholePort,
        }
    }

    pub fn hostname(
        port: u16,
        transport: ClaimTransport,
        kind: ClaimKind,
        hostname: impl Into<String>,
    ) -> Self {
        Self {
            port,
            transport,
            scope: ClaimScope::Hostname {
                kind,
                hostname: hostname.into(),
            },
        }
    }

    /// Whether the two claims cannot both be served.
    ///
    /// Claims on the same port and transport are served together only when
    /// both are for a hostname, are of one kind, and name different hostnames.
    // spec: PRX#shared-ports
    pub fn conflicts_with(&self, other: &Claim) -> bool {
        if self.port != other.port || self.transport != other.transport {
            return false;
        }
        match (&self.scope, &other.scope) {
            (
                ClaimScope::Hostname {
                    kind: a_kind,
                    hostname: a_host,
                },
                ClaimScope::Hostname {
                    kind: b_kind,
                    hostname: b_host,
                },
            ) => a_kind != b_kind || a_host == b_host,
            _ => true,
        }
    }

    /// The hostname the claim is for, if it is for one.
    pub fn claimed_hostname(&self) -> Option<&str> {
        match &self.scope {
            ClaimScope::WholePort => None,
            ClaimScope::Hostname { hostname, .. } => Some(hostname),
        }
    }
}

impl std::fmt::Display for Claim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.transport, self.port)?;
        match &self.scope {
            ClaimScope::WholePort => f.write_str(" (whole port)"),
            ClaimScope::Hostname { kind, hostname } => write!(f, " ({kind} {hostname})"),
        }
    }
}

impl IngressDef {
    /// The claims the ingress itself makes, one per transport it serves.
    // spec: PRX#shared-ports
    pub fn claims(&self) -> Vec<Claim> {
        let port = self.port.get();
        let hostname = self.hostname.as_str();
        if self.dtls {
            return vec![Claim::whole_port(port, ClaimTransport::Udp)];
        }
        match (self.tls, self.http_terminate) {
            (true, Some(_)) => vec![
                Claim::hostname(port, ClaimTransport::Tcp, ClaimKind::Tls, hostname),
                Claim::hostname(port, ClaimTransport::Udp, ClaimKind::Http3, hostname),
            ],
            (true, None) => vec![Claim::hostname(
                port,
                ClaimTransport::Tcp,
                ClaimKind::Tls,
                hostname,
            )],
            (false, Some(_)) => vec![Claim::hostname(
                port,
                ClaimTransport::Tcp,
                ClaimKind::PlaintextHttp,
                hostname,
            )],
            (false, None) => vec![Claim::whole_port(port, ClaimTransport::Tcp)],
        }
    }

    /// The claim of the ingress's HTTP redirect: plaintext HTTP on the
    /// redirect's port.
    // spec: PRX#shared-ports
    pub fn redirect_claim(&self) -> Option<Claim> {
        self.redirect.as_ref().map(|redirect| {
            Claim::hostname(
                redirect.port.get(),
                ClaimTransport::Tcp,
                ClaimKind::PlaintextHttp,
                self.hostname.as_str(),
            )
        })
    }
}

/// Refuse a definition in which two ingresses make conflicting claims on a
/// port. The redirect of an ingress is a claim of the ingress, and is held to
/// the ingress's own claims as well: a redirect on the ingress's own port
/// would bind that port for plaintext and TLS at once.
// spec: ING#identity-within-an-app
pub(crate) fn check_claim_conflicts(def: &super::app::AppDef) -> Result<(), String> {
    // (ingress name, whether the claim is its redirect, the claim)
    let mut claimed: Vec<(String, bool, Claim)> = Vec::new();
    for resource in def.resources.values() {
        let super::resource::Resource::Ingress(ingress) = resource else {
            continue;
        };
        let ingress_def = ingress.def.lock();
        for claim in ingress_def.claims() {
            claimed.push((ingress.name.to_string(), false, claim));
        }
        if let Some(claim) = ingress_def.redirect_claim() {
            claimed.push((ingress.name.to_string(), true, claim));
        }
    }
    claimed.sort();
    for (i, (name, redirect, claim)) in claimed.iter().enumerate() {
        for (other_name, other_redirect, other_claim) in &claimed[i + 1..] {
            // One ingress's own claims are one per transport, and never
            // conflict among themselves.
            let same_ingress_core = name == other_name && !redirect && !other_redirect;
            if same_ingress_core || !claim.conflicts_with(other_claim) {
                continue;
            }
            return Err(if name == other_name {
                format!(
                    "ingress '{name}' declares a redirect that conflicts with the ingress \
                     itself on {}/{}: {claim} against {other_claim}",
                    claim.transport, claim.port
                )
            } else {
                format!(
                    "ingresses '{name}' and '{other_name}' make conflicting claims on {}/{}: \
                     {claim} against {other_claim}",
                    claim.transport, claim.port
                )
            });
        }
    }
    Ok(())
}

/// Apply a `(Terminate, Output)` pair to the ingress's `IngressDef`.
/// Only the four pairings that match a real protocol stack are
/// accepted; everything else throws so a script that asks for, say,
/// `Terminate.Https` with `Output.Tcp` doesn't silently produce a
/// nonsensical ingress.
// spec: ING#termination
fn apply_termination(
    def: &mut IngressDef,
    terminate: Terminate,
    output: Output,
) -> Result<(), Box<EvalAltResult>> {
    match (terminate, output) {
        (Terminate::Tls, Output::Tcp) => {
            def.tls = true;
            def.dtls = false;
            def.http_terminate = None;
        }
        (Terminate::Dtls, Output::Udp) => {
            def.tls = false;
            def.dtls = true;
            def.http_terminate = None;
        }
        (Terminate::Https, Output::Http1) => {
            def.tls = true;
            def.dtls = false;
            def.http_terminate = Some(HttpTermination::Http1);
        }
        (Terminate::Https, Output::Http2) => {
            def.tls = true;
            def.dtls = false;
            def.http_terminate = Some(HttpTermination::Http2);
        }
        _ => {
            return Err(format!(
                "invalid termination/output combination: \
                 Terminate.{terminate:?} + Output.{output:?}. \
                 Valid combinations are Terminate.Tls + Output.Tcp, \
                 Terminate.Dtls + Output.Udp, \
                 Terminate.Https + Output.Http1, and \
                 Terminate.Https + Output.Http2."
            )
            .into());
        }
    }
    Ok(())
}

impl CustomType for Ingress {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Ingress")
            // spec: ING#termination
            .with_fn(
                "tls",
                |this: &mut Self,
                 terminate: Terminate,
                 output: Output|
                 -> Result<Ingress, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let mut def = this.def.lock();
                    apply_termination(&mut def, terminate, output)?;
                    drop(def);
                    Ok(this.clone())
                },
            )
            // spec: ING#redirect-to-https
            .with_fn(
                "redirect",
                |this: &mut Self| -> Result<Ingress, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().redirect = Some(RedirectDef {
                        port: Port::from_u16(80),
                        code: 307,
                    });
                    Ok(this.clone())
                },
            )
            .with_fn(
                "redirect",
                |this: &mut Self, port: i64| -> Result<Ingress, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let port = Port::new(port)?;
                    this.def.lock().redirect = Some(RedirectDef { port, code: 307 });
                    Ok(this.clone())
                },
            )
            .with_fn(
                "redirect",
                |this: &mut Self, port: i64, code: i64| -> Result<Ingress, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let port = Port::new(port)?;
                    // spec: ING#redirect-to-https
                    let code = redirect_code(code)?;
                    this.def.lock().redirect = Some(RedirectDef { port, code });
                    Ok(this.clone())
                },
            )
            // spec: ING#declaration
            .with_fn("service", |this: &mut Self| this.service.clone())
            // spec: BSL#descriptions
            .with_fn(
                "description",
                |this: &mut Self, desc: &str| -> Result<Ingress, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().description = Some(desc.to_owned());
                    Ok(this.clone())
                },
            );
    }
}
