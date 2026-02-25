use rhai::{CustomType, TypeBuilder};

use super::{Holder, service::Service};

#[derive(Debug, Clone)]
pub struct IngressDef {
    pub host: Option<String>,
    pub tls: bool,
    pub service: Service,
}

impl IngressDef {
    pub(super) fn new(service: Service) -> Self {
        Self {
            host: None,
            tls: false,
            service,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Ingress(Holder<IngressDef>);

impl Ingress {
    pub(super) fn new(def: IngressDef) -> Self {
        Self(Holder::new(def.into()))
    }
}

impl CustomType for Ingress {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Ingress")
            .with_fn("host", |this: &mut Self, host: &str| {
                this.0.lock().host = Some(host.into());
                this.clone()
            })
            .with_fn("tls", |this: &mut Self| {
                this.0.lock().tls = true;
                this.clone()
            });
    }
}
