use rhai::{CustomType, TypeBuilder};

use super::{Holder, resource::ResourceName, service::Service};

// r[ingress.type]
#[derive(Debug, Clone)]
pub struct IngressDef {
    pub hostname: String,
    pub port: u16,
    pub tls: bool,
    pub dtls: bool,
    pub quic: bool,
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
    pub port: u16,
    pub code: u16,
}

#[derive(Debug, Clone)]
pub struct Ingress {
    pub name: ResourceName,
    pub service: Service,
    pub def: Holder<IngressDef>,
}

impl Ingress {
    pub fn new(service: Service, hostname: String, port: u16) -> Self {
        let name = service.name.clone();
        Self {
            name,
            service: service.clone(),
            def: Holder::new(
                IngressDef {
                    hostname,
                    port,
                    tls: false,
                    dtls: false,
                    quic: false,
                    http_terminate: None,
                    redirect: None,
                }
                .into(),
            ),
        }
    }
}

// r[ingress.tls]
// r[ingress.dtls]
// r[ingress.quic]
// r[ingress.http]
// r[ingress.http2]
// r[ingress.redirect]
// r[ingress.service]
impl CustomType for Ingress {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Ingress")
            .with_fn("tls", |this: &mut Self| {
                this.def.lock().tls = true;
                this.clone()
            })
            .with_fn("dtls", |this: &mut Self| {
                this.def.lock().dtls = true;
                this.clone()
            })
            .with_fn("quic", |this: &mut Self| {
                this.def.lock().quic = true;
                this.clone()
            })
            .with_fn("http", |this: &mut Self| {
                {
                    let mut def = this.def.lock();
                    def.tls = true;
                    def.http_terminate = Some(HttpTermination::Http1);
                }
                this.clone()
            })
            .with_fn("http2", |this: &mut Self| {
                {
                    let mut def = this.def.lock();
                    def.tls = true;
                    def.http_terminate = Some(HttpTermination::Http2);
                }
                this.clone()
            })
            .with_fn("redirect", |this: &mut Self| {
                let def = this.def.lock();
                if def.http_terminate.is_none() {
                    drop(def);
                    panic!("redirect() requires an HTTPS termination (http() or http2())");
                }
                drop(def);
                this.def.lock().redirect = Some(RedirectDef {
                    port: 80,
                    code: 307,
                });
                this.clone()
            })
            .with_fn("redirect", |this: &mut Self, port: i64| {
                let def = this.def.lock();
                if def.http_terminate.is_none() {
                    drop(def);
                    panic!("redirect() requires an HTTPS termination (http() or http2())");
                }
                drop(def);
                this.def.lock().redirect = Some(RedirectDef {
                    port: port as u16,
                    code: 307,
                });
                this.clone()
            })
            .with_fn("redirect", |this: &mut Self, port: i64, code: i64| {
                let def = this.def.lock();
                if def.http_terminate.is_none() {
                    drop(def);
                    panic!("redirect() requires an HTTPS termination (http() or http2())");
                }
                drop(def);
                this.def.lock().redirect = Some(RedirectDef {
                    port: port as u16,
                    code: code as u16,
                });
                this.clone()
            })
            .with_fn("service", |this: &mut Self| this.service.clone());
    }
}
