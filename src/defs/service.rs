use rhai::{CustomType, TypeBuilder};

use super::{Holder, resource::ResourceName};

pub type ResourcePort = (ResourceName, u16);
pub type ServiceHttpRoute = (u16, String);

// r[service.type]
#[derive(Debug, Default, Clone)]
pub struct ServiceDef {
    pub http: Option<HttpServiceDef>,
}

#[derive(Debug, Clone)]
pub struct Service {
    pub name: ResourceName,
    pub def: Holder<ServiceDef>,
}

impl Service {
    pub fn new(name: ResourceName) -> Self {
        Self {
            name,
            def: Default::default(),
        }
    }
}

// r[service.port]
// r[service.http]
// r[service.routing]
impl CustomType for Service {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Service")
            .with_fn("port", |this: &mut Self, port: i64| {
                validate_port(port);
                ServicePort {
                    service: this.clone(),
                    port: port as u16,
                }
            })
            .with_fn("http", |this: &mut Self| {
                this.def.lock().http.get_or_insert_default();
                HttpService {
                    service: this.clone(),
                    port: 80,
                }
            })
            .with_fn("http", |this: &mut Self, port: i64| {
                validate_port(port);
                this.def.lock().http.get_or_insert_default();
                HttpService {
                    service: this.clone(),
                    port: port as u16,
                }
            })
            .with_fn("ingress", |this: &mut Self, hostname: &str, port: i64| {
                validate_port(port);
                IngressBuilder {
                    service: this.clone(),
                    hostname: hostname.into(),
                    port: port as u16,
                }
            });
    }
}

// r[service.port]
#[derive(Debug, Clone)]
pub struct ServicePort {
    pub service: Service,
    pub port: u16,
}

impl CustomType for ServicePort {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("ServicePort");
    }
}

// r[service.http]
#[derive(Debug, Default, Clone)]
pub struct HttpServiceDef {}

#[derive(Debug, Clone)]
pub struct HttpService {
    pub service: Service,
    pub port: u16,
}

// r[service.http.route]
impl CustomType for HttpService {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("HttpService")
            .with_fn("route", |this: &mut Self, prefix: &str| {
                if prefix.is_empty() || !prefix.starts_with('/') {
                    panic!("route prefix must be a non-empty string starting with '/'");
                }
                HttpServiceRoute {
                    http: this.clone(),
                    prefix: prefix.into(),
                }
            })
            .with_fn("port", |this: &mut Self, port: i64| {
                validate_port(port);
                ServicePort {
                    service: this.service.clone(),
                    port: port as u16,
                }
            });
    }
}

// r[service.http.route]
#[derive(Debug, Clone)]
pub struct HttpServiceRoute {
    pub http: HttpService,
    pub prefix: String,
}

impl CustomType for HttpServiceRoute {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("HttpServiceRoute");
    }
}

// r[service.external]
#[derive(Debug, Clone)]
pub struct ExternalService {
    pub name: ResourceName,
}

// r[service.external.port]
impl CustomType for ExternalService {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("ExternalService")
            .with_fn("port", |this: &mut Self, port: i64| {
                validate_port(port);
                ServicePort {
                    service: Service::new(this.name.clone()),
                    port: port as u16,
                }
            });
    }
}

// r[ingress.type]
#[derive(Debug, Clone)]
pub struct IngressBuilder {
    pub service: Service,
    pub hostname: String,
    pub port: u16,
}

impl CustomType for IngressBuilder {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("IngressBuilder");
    }
}

// r[bsl.port]
fn validate_port(port: i64) {
    if port <= 0 || port >= 65535 {
        panic!("port must be a non-zero positive integer below 65535, got {port}");
    }
}
