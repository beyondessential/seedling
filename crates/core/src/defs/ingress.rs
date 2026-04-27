use std::sync::Arc;

use rhai::{CustomType, EvalAltResult, TypeBuilder};

use super::{Freezable, Holder, Port, resource::ResourceName, service::Service};

#[derive(Debug, Clone)]
pub struct IngressDef {
    pub hostname: String,
    pub port: Port,
    pub tls: bool,
    pub dtls: bool,
    pub http_terminate: Option<HttpTermination>,
    pub redirect: Option<RedirectDef>,
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
        return Err("redirect() requires an HTTPS termination (http() or http2())".into());
    }
    Ok(())
}

impl CustomType for Ingress {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Ingress")
            // l[impl ingress.tls]
            .with_fn(
                "tls",
                |this: &mut Self| -> Result<Ingress, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().tls = true;
                    Ok(this.clone())
                },
            )
            // l[impl ingress.dtls]
            .with_fn(
                "dtls",
                |this: &mut Self| -> Result<Ingress, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().dtls = true;
                    Ok(this.clone())
                },
            )
            // l[impl ingress.http]
            .with_fn(
                "http",
                |this: &mut Self| -> Result<Ingress, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    {
                        let mut def = this.def.lock();
                        def.tls = true;
                        def.http_terminate = Some(HttpTermination::Http1);
                    }
                    Ok(this.clone())
                },
            )
            // l[impl ingress.http2]
            .with_fn(
                "http2",
                |this: &mut Self| -> Result<Ingress, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    {
                        let mut def = this.def.lock();
                        def.tls = true;
                        def.http_terminate = Some(HttpTermination::Http2);
                    }
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
            .with_fn("service", |this: &mut Self| this.service.clone());
    }
}
