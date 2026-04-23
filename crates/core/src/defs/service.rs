use std::sync::Weak;

use rhai::{CustomType, EvalAltResult, Map, TypeBuilder};

use super::{
    Freezable, Holder, Port,
    app::AppDef,
    export::ExportOptions,
    ingress::Ingress,
    resource::{Resource, ResourceId, ResourceKind, ResourceName},
};

// l[impl service.type]
#[derive(Debug, Default, Clone)]
pub struct ServiceDef {
    pub http: Option<HttpServiceDef>,
    pub exported: Option<ExportOptions>,
}

#[derive(Debug, Clone)]
pub struct Service {
    pub name: ResourceName,
    pub def: Holder<ServiceDef>,
    /// Weak back-reference to the owning `AppDef` so that `ingress()` can
    /// register the created `Ingress` into `app_def.resources`.
    pub(super) app_def: Option<Weak<arc_swap::ArcSwap<AppDef>>>,
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

    pub(super) fn new_with_app(
        name: ResourceName,
        app_def: Weak<arc_swap::ArcSwap<AppDef>>,
    ) -> Self {
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
                    let port = Port::new(port)?;
                    Ok(ServicePort {
                        service: this.clone(),
                        port,
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
                        port: Port::from_u16(80),
                    })
                },
            )
            .with_fn(
                "http",
                |this: &mut Self, port: i64| -> Result<HttpService, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let port = Port::new(port)?;
                    this.def.lock().http.get_or_insert_default();
                    Ok(HttpService {
                        service: this.clone(),
                        port,
                    })
                },
            )
            // l[impl service.exported]
            .with_fn(
                "exported",
                |this: &mut Self| -> Result<Service, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    if this.name.as_str().is_empty() {
                        return Err("only named services can be exported".into());
                    }
                    this.def.lock().exported = Some(ExportOptions::default());
                    Ok(this.clone())
                },
            )
            // l[impl service.exported]
            .with_fn(
                "exported",
                |this: &mut Self, options: Map| -> Result<Service, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    if this.name.as_str().is_empty() {
                        return Err("only named services can be exported".into());
                    }
                    this.def.lock().exported = Some(ExportOptions::from_rhai_map(options)?);
                    Ok(this.clone())
                },
            )
            .with_fn(
                "ingress",
                |this: &mut Self,
                 hostname: &str,
                 port: i64|
                 -> Result<Ingress, Box<EvalAltResult>> {
                    this.ensure_unfrozen()?;
                    let port = Port::new(port)?;
                    validate_hostname(hostname)?;
                    // l[impl ingress.conflicts]
                    // TODO: check for duplicate (hostname, port) in the app's ingress
                    // registry and throw if a conflict is found.
                    let ingress = Ingress::new(this.clone(), hostname.into(), port);
                    // l[impl ingress.type]
                    if let Some(arc) = this.app_def.as_ref().and_then(Weak::upgrade) {
                        let id = ResourceId {
                            kind: ResourceKind::Ingress,
                            name: ingress.name.clone(),
                        };
                        let ingress_clone = ingress.clone();
                        arc.rcu(|d| {
                            let mut d = (**d).clone();
                            d.resources
                                .insert(id.clone(), Resource::Ingress(ingress_clone.clone()));
                            d
                        });
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
    pub port: Port,
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
    pub port: Port,
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
                    let port = Port::new(port)?;
                    Ok(ServicePort {
                        service: this.service.clone(),
                        port,
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
        builder.with_name("ExternalService");
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
