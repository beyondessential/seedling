use std::sync::Arc;

use rhai::{CustomType, EvalAltResult, TypeBuilder};

use crate::runtime::barrier::runtime::is_in_action_closure;

use super::{
    Freezable, Holder, Port,
    enums::{Output, Terminate},
    resource::ResourceName,
    service::Service,
    take::take_int_in_range,
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
                    // spec: ING#redirect-to-https — a redirect to a non-3xx code
                    // is not a redirect, and the unchecked cast turned 65843
                    // into a 307 rather than an error.
                    let code = take_int_in_range::<u16>("redirect code", code, 300..=399)?;
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
