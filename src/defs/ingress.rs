use rhai::{CustomType, TypeBuilder};

use super::{Holder, service::ServiceDef};

#[derive(Debug, Default, Clone)]
pub struct IngressDef {
    pub host: Option<String>,
    pub tls: bool,
    pub service: ServiceDef,
}

#[derive(Debug, Default, Clone)]
pub struct Ingress(Holder<IngressDef>);

impl CustomType for Ingress {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Ingress")
            .with_fn("host", |this: &mut Self, host: &str| {
                this.0.lock().unwrap().host = Some(host.into());
                this.clone()
            })
            .with_fn("tls", |this: &mut Self| {
                this.0.lock().unwrap().tls = true;
                this.clone()
            })
            .with_fn("http", |this: &mut Self| {
                this.0.lock().unwrap().service.make_http();
                this.clone()
            });
    }
}
