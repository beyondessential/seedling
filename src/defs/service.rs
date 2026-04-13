use std::sync::Weak;

use parking_lot::Mutex;
use rhai::{CustomType, EvalAltResult, TypeBuilder};

use super::{
    Freezable, Holder,
    app::AppDef,
    ingress::Ingress,
    resource::{Resource, ResourceId, ResourceKind, ResourceName},
};

// l[impl service.type]
#[derive(Debug, Default, Clone)]
pub struct ServiceDef {
    pub http: Option<HttpServiceDef>,
}

#[derive(Debug, Clone)]
pub struct Service {
    pub name: ResourceName,
    pub def: Holder<ServiceDef>,
    /// Weak back-reference to the owning `AppDef` so that `ingress()` can
    /// register the created `Ingress` into `app_def.resources`. `None` for
    /// services created outside of an `App` context (e.g. via `ExternalService`).
    pub(super) app_def: Option<Weak<Mutex<AppDef>>>,
    pub frozen: bool,
}

impl super::Freezable for Service {
    fn is_frozen(&self) -> bool {
        self.frozen
    }
}

impl Service {
    pub fn new(name: ResourceName) -> Self {
        Self {
            name,
            def: Default::default(),
            app_def: None,
            frozen: false,
        }
    }

    pub(super) fn new_with_app(name: ResourceName, app_def: Weak<Mutex<AppDef>>) -> Self {
        Self {
            name,
            def: Default::default(),
            app_def: Some(app_def),
            frozen: false,
        }
    }
}

impl CustomType for Service {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Service")
            // l[impl service.port]
            .with_fn(
                "port",
                |this: &mut Self, port: i64| -> Result<ServicePort, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    validate_port(port)?;
                    Ok(ServicePort {
                        service: this.clone(),
                        port: port as u16,
                    })
                },
            )
            // l[impl service.http]
            .with_fn(
                "http",
                |this: &mut Self| -> Result<HttpService, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    this.def.lock().http.get_or_insert_default();
                    Ok(HttpService {
                        service: this.clone(),
                        port: 80,
                    })
                },
            )
            .with_fn(
                "http",
                |this: &mut Self, port: i64| -> Result<HttpService, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    validate_port(port)?;
                    this.def.lock().http.get_or_insert_default();
                    Ok(HttpService {
                        service: this.clone(),
                        port: port as u16,
                    })
                },
            )
            .with_fn(
                "ingress",
                |this: &mut Self,
                 hostname: &str,
                 port: i64|
                 -> Result<Ingress, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    validate_port(port)?;
                    validate_hostname(hostname)?;
                    // l[impl ingress.conflicts]
                    // TODO: check for duplicate (hostname, port) in the app's ingress
                    // registry and throw if a conflict is found.
                    let ingress = Ingress::new(this.clone(), hostname.into(), port as u16);
                    // l[impl ingress.type]
                    if let Some(arc) = this.app_def.as_ref().and_then(Weak::upgrade) {
                        let id = ResourceId {
                            kind: ResourceKind::Ingress,
                            name: ingress.name.clone(),
                        };
                        arc.lock()
                            .resources
                            .insert(id, Resource::Ingress(ingress.clone()));
                    }
                    Ok(ingress)
                },
            );
    }
}

// l[impl service.port]
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

// l[impl service.http]
#[derive(Debug, Default, Clone)]
pub struct HttpServiceDef {}

#[derive(Debug, Clone)]
pub struct HttpService {
    pub service: Service,
    pub port: u16,
}

impl CustomType for HttpService {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("HttpService")
            // l[impl service.http.route]
            .with_fn(
                "route",
                |this: &mut Self, prefix: &str| -> Result<HttpServiceRoute, Box<EvalAltResult>> {
                    if prefix.is_empty() || !prefix.starts_with('/') {
                        return Err(
                            "route prefix must be a non-empty string starting with '/'".into()
                        );
                    }
                    Ok(HttpServiceRoute {
                        http: this.clone(),
                        prefix: prefix.into(),
                    })
                },
            )
            .with_fn(
                "port",
                |this: &mut Self, port: i64| -> Result<ServicePort, Box<EvalAltResult>> {
                    validate_port(port)?;
                    Ok(ServicePort {
                        service: this.service.clone(),
                        port: port as u16,
                    })
                },
            );
    }
}

// l[impl service.http.route]
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

// l[impl service.external]
#[derive(Debug, Clone)]
pub struct ExternalService {
    pub name: ResourceName,
}

impl CustomType for ExternalService {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("ExternalService")
            // l[impl service.external.port]
            .with_fn(
                "port",
                |this: &mut Self, port: i64| -> Result<ServicePort, Box<EvalAltResult>> {
                    validate_port(port)?;
                    Ok(ServicePort {
                        service: Service::new(this.name.clone()),
                        port: port as u16,
                    })
                },
            );
    }
}

// l[impl bsl.port]
fn validate_port(port: i64) -> Result<(), Box<EvalAltResult>> {
    if port <= 0 || port >= 65535 {
        Err(format!("port must be a non-zero positive integer below 65535, got {port}").into())
    } else {
        Ok(())
    }
}

// l[impl ingress.hostname]
fn validate_hostname(hostname: &str) -> Result<(), Box<EvalAltResult>> {
    if hostname.is_empty() || hostname.len() > 253 {
        return Err(format!("hostname must be 1–253 characters, got {}", hostname.len()).into());
    }

    if hostname.contains('*') {
        return Err("wildcard hostnames are not permitted".into());
    }

    for label in hostname.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(format!(
                "each hostname label must be 1–63 characters, got '{}' ({})",
                label,
                label.len()
            )
            .into());
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(
                format!("hostname label must not start or end with a hyphen: '{label}'").into(),
            );
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(format!("hostname label contains invalid characters: '{label}'").into());
        }
    }

    Ok(())
}
