use std::sync::Arc;

use rhai::{CustomType, EvalAltResult, TypeBuilder};

use super::{
    Freezable, Holder, Port,
    enums::{Output, Terminate},
    resource::ResourceName,
    service::Service,
};

#[derive(Debug, Clone)]
pub struct IngressDef {
    pub hostname: String,
    pub port: Port,
    pub tls: bool,
    pub dtls: bool,
    pub http_terminate: Option<HttpTermination>,
    pub redirect: Option<RedirectDef>,
    // l[impl bsl.resource.description]
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

// l[impl ingress.type]
#[derive(Debug, Clone)]
pub struct Ingress {
    pub name: ResourceName,
    pub service: Service,
    pub def: Holder<IngressDef>,
    pub frozen: bool,
}

impl super::Freezable for Ingress {
    fn is_frozen(&self) -> bool {
        self.frozen
    }
}

impl Ingress {
    pub fn new(service: Service, hostname: String, port: Port) -> Self {
        // The ingress's resource name is its (hostname, port) tuple,
        // not the underlying service's name: a single service is
        // allowed multiple ingresses (different hostnames, redirect
        // settings, ports), and they need distinct identities so the
        // app's resource map doesn't collapse them.
        // l[impl ingress.type]
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

fn require_https(def: &IngressDef) -> Result<(), Box<EvalAltResult>> {
    if def.http_terminate.is_none() {
        return Err(
            "redirect() requires an HTTPS termination, e.g. tls(Terminate.Https, Output.Http1)"
                .into(),
        );
    }
    Ok(())
}

/// Apply a `(Terminate, Output)` pair to the ingress's `IngressDef`.
/// Only the four pairings that match a real protocol stack are
/// accepted; everything else throws so a script that asks for, say,
/// `Terminate.Https` with `Output.Tcp` doesn't silently produce a
/// nonsensical ingress.
// l[impl ingress.termination]
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
            // l[impl ingress.termination]
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
            // l[impl ingress.redirect]
            .with_fn(
                "redirect",
                |this: &mut Self| -> Result<Ingress, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    require_https(&this.def.lock())?;
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
                    require_https(&this.def.lock())?;
                    this.def.lock().redirect = Some(RedirectDef { port, code: 307 });
                    Ok(this.clone())
                },
            )
            .with_fn(
                "redirect",
                |this: &mut Self, port: i64, code: i64| -> Result<Ingress, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let port = Port::new(port)?;
                    require_https(&this.def.lock())?;
                    this.def.lock().redirect = Some(RedirectDef {
                        port,
                        code: code as u16,
                    });
                    Ok(this.clone())
                },
            )
            // l[impl ingress.service]
            .with_fn("service", |this: &mut Self| this.service.clone())
            // l[impl bsl.resource.description]
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
