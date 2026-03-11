use std::collections::BTreeMap;

use rhai::{CustomType, Dynamic, FnPtr, Map, TypeBuilder};

use super::{
    Holder,
    action::{Action, ActionDef, ShellDef},
    collection::Collection,
    deployment::Deployment,
    install::{InstallDef, InstallRequirementDef, InstallRequirementKind},
    job::Job,
    resource::{Resource, ResourceId, ResourceKind, ResourceName},
    service::{ExternalService, Service},
    volume::{ExternalVolume, Volume},
};

// l[impl app.type]
// l[impl app.var]
// l[impl app.constructor]
#[derive(Debug, Default, Clone)]
pub struct App(pub Holder<AppDef>);

#[derive(Debug, Default, Clone)]
pub struct AppDef {
    pub params: BTreeMap<String, String>,
    pub resources: BTreeMap<ResourceId, Resource>,
    pub actions: BTreeMap<String, ActionDef>,
    pub shells: BTreeMap<String, ShellDef>,
    pub install: Option<InstallDef>,
}

fn extract_description(options: &Map) -> Option<String> {
    options
        .get("description")
        .and_then(|v| v.clone().into_string().ok())
}

// l[impl app.methods]
// l[impl app.resources]
// l[impl app.resources.names]
// l[impl app.resources.static]
// l[impl app.resources.dynamic]
impl CustomType for App {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("App");

        // l[impl param.type]
        builder.with_fn("param", |this: &mut Self, name: &str| -> Dynamic {
            let mut def = this.0.lock();
            let value = def
                .params
                .entry(name.into())
                .or_insert_with(|| "<placeholder>".into())
                .clone();
            Dynamic::from(value)
        });

        // l[impl service.type]
        builder.with_fn("service", |this: &mut Self, name: &str| -> Service {
            let name = ResourceName::new(name.into());
            let mut def = this.0.lock();
            let id = ResourceId {
                kind: ResourceKind::Service,
                name: name.clone(),
            };
            let resource = def
                .resources
                .entry(id)
                .or_insert_with(|| Resource::Service(Service::new(name)));
            match resource {
                Resource::Service(s) => s.clone(),
                _ => unreachable!(),
            }
        });

        // l[impl service.external]
        builder.with_fn(
            "external_service",
            |this: &mut Self, name: &str| -> Dynamic {
                let rname = ResourceName::new(name.into());
                let mut def = this.0.lock();
                let id = ResourceId {
                    kind: ResourceKind::ExternalService,
                    name: rname.clone(),
                };
                let resource = def
                    .resources
                    .entry(id)
                    .or_insert_with(|| Resource::ExternalService(ExternalService { name: rname }));
                match resource {
                    Resource::ExternalService(s) => Dynamic::from(s.clone()),
                    _ => unreachable!(),
                }
            },
        );

        // l[impl deployment.type]
        builder.with_fn("deployment", |this: &mut Self, name: &str| -> Deployment {
            let name = ResourceName::new(name.into());
            let mut def = this.0.lock();
            let id = ResourceId {
                kind: ResourceKind::Deployment,
                name: name.clone(),
            };
            let resource = def.resources.entry(id).or_insert_with(|| {
                Resource::Deployment(Deployment {
                    name,
                    def: Default::default(),
                })
            });
            match resource {
                Resource::Deployment(d) => d.clone(),
                _ => unreachable!(),
            }
        });

        // l[impl job.type]
        builder.with_fn("job", |this: &mut Self, name: &str| -> Job {
            let name = ResourceName::new(name.into());
            let mut def = this.0.lock();
            let id = ResourceId {
                kind: ResourceKind::Job,
                name: name.clone(),
            };
            let resource = def.resources.entry(id).or_insert_with(|| {
                Resource::Job(Job {
                    name,
                    def: Default::default(),
                })
            });
            match resource {
                Resource::Job(j) => j.clone(),
                _ => unreachable!(),
            }
        });

        // l[impl volume.type] — named volume
        builder.with_fn("volume", |this: &mut Self, name: &str| -> Volume {
            let rname = ResourceName::new(name.into());
            let mut def = this.0.lock();
            let id = ResourceId {
                kind: ResourceKind::Volume,
                name: rname.clone(),
            };
            let resource = def
                .resources
                .entry(id)
                .or_insert_with(|| Resource::Volume(Volume::new(Some(rname))));
            match resource {
                Resource::Volume(v) => v.clone(),
                _ => unreachable!(),
            }
        });

        // l[impl volume.type] — anonymous volume
        builder.with_fn("volume", |_this: &mut Self| -> Volume { Volume::new(None) });

        // l[impl volume.external]
        builder.with_fn(
            "external_volume",
            |this: &mut Self, name: &str| -> Dynamic {
                let rname = ResourceName::new(name.into());
                let mut def = this.0.lock();
                let id = ResourceId {
                    kind: ResourceKind::ExternalVolume,
                    name: rname.clone(),
                };
                let resource = def
                    .resources
                    .entry(id)
                    .or_insert_with(|| Resource::ExternalVolume(ExternalVolume { name: rname }));
                match resource {
                    Resource::ExternalVolume(v) => Dynamic::from(v.clone()),
                    _ => unreachable!(),
                }
            },
        );

        // Ingress creation: service.ingress(hostname, port) returns IngressBuilder,
        // which needs to be registered against the app. We handle this by making
        // IngressBuilder chain methods that ultimately produce an Ingress on the app.
        // However, since IngressBuilder doesn't have access to App, we store the
        // ingress on the service's app reference. Instead, we register ingress
        // creation directly through the service, but the Ingress resource is created
        // lazily. For now, we also provide a way to "finish" an IngressBuilder.

        builder.with_fn(
            "select",
            |_this: &mut Self, _criterion: Map| -> Collection { Collection },
        );

        builder.with_fn("one", |_this: &mut Self| -> Collection { Collection });

        builder.with_fn("only", |_this: &mut Self, _other: Dynamic| -> Collection {
            Collection
        });

        builder.with_fn(
            "except",
            |_this: &mut Self, _other: Dynamic| -> Collection { Collection },
        );

        // l[impl action.type]
        // l[impl action.option-description]
        builder
            .with_fn(
                "on_action",
                |this: &mut Self, name: &str, closure: FnPtr| -> Action {
                    let mut def = this.0.lock();
                    def.actions.insert(
                        name.into(),
                        ActionDef {
                            name: name.into(),
                            closure,
                            description: None,
                        },
                    );
                    Action { name: name.into() }
                },
            )
            .with_fn(
                "on_action",
                |this: &mut Self, name: &str, closure: FnPtr, options: Map| -> Action {
                    let desc = extract_description(&options);
                    let mut def = this.0.lock();
                    def.actions.insert(
                        name.into(),
                        ActionDef {
                            name: name.into(),
                            closure,
                            description: desc,
                        },
                    );
                    Action { name: name.into() }
                },
            );

        // l[impl action.start]
        builder
            .with_fn("on_start", |this: &mut Self, closure: FnPtr| -> Action {
                let mut def = this.0.lock();
                def.actions.insert(
                    "start".into(),
                    ActionDef {
                        name: "start".into(),
                        closure,
                        description: None,
                    },
                );
                Action {
                    name: "start".into(),
                }
            })
            .with_fn(
                "on_start",
                |this: &mut Self, closure: FnPtr, options: Map| -> Action {
                    let desc = extract_description(&options);
                    let mut def = this.0.lock();
                    def.actions.insert(
                        "start".into(),
                        ActionDef {
                            name: "start".into(),
                            closure,
                            description: desc,
                        },
                    );
                    Action {
                        name: "start".into(),
                    }
                },
            );

        // l[impl action.upgrade]
        builder
            .with_fn("on_upgrade", |this: &mut Self, closure: FnPtr| -> Action {
                let mut def = this.0.lock();
                def.actions.insert(
                    "upgrade".into(),
                    ActionDef {
                        name: "upgrade".into(),
                        closure,
                        description: None,
                    },
                );
                Action {
                    name: "upgrade".into(),
                }
            })
            .with_fn(
                "on_upgrade",
                |this: &mut Self, closure: FnPtr, options: Map| -> Action {
                    let desc = extract_description(&options);
                    let mut def = this.0.lock();
                    def.actions.insert(
                        "upgrade".into(),
                        ActionDef {
                            name: "upgrade".into(),
                            closure,
                            description: desc,
                        },
                    );
                    Action {
                        name: "upgrade".into(),
                    }
                },
            );

        // l[impl action.crash-recovery]
        builder
            .with_fn(
                "on_crash_recovery",
                |this: &mut Self, closure: FnPtr| -> Action {
                    let mut def = this.0.lock();
                    def.actions.insert(
                        "crash_recovery".into(),
                        ActionDef {
                            name: "crash_recovery".into(),
                            closure,
                            description: None,
                        },
                    );
                    Action {
                        name: "crash_recovery".into(),
                    }
                },
            )
            .with_fn(
                "on_crash_recovery",
                |this: &mut Self, closure: FnPtr, options: Map| -> Action {
                    let desc = extract_description(&options);
                    let mut def = this.0.lock();
                    def.actions.insert(
                        "crash_recovery".into(),
                        ActionDef {
                            name: "crash_recovery".into(),
                            closure,
                            description: desc,
                        },
                    );
                    Action {
                        name: "crash_recovery".into(),
                    }
                },
            );

        // l[impl action.shell]
        builder
            .with_fn("on_shell", |this: &mut Self, name: &str, closure: FnPtr| {
                let mut def = this.0.lock();
                def.shells.insert(
                    name.into(),
                    ShellDef {
                        name: name.into(),
                        closure,
                        description: None,
                    },
                );
            })
            .with_fn(
                "on_shell",
                |this: &mut Self, name: &str, closure: FnPtr, options: Map| {
                    let desc = extract_description(&options);
                    let mut def = this.0.lock();
                    def.shells.insert(
                        name.into(),
                        ShellDef {
                            name: name.into(),
                            closure,
                            description: desc,
                        },
                    );
                },
            );

        // l[impl action.install]
        builder
            .with_fn("on_install", |this: &mut Self, closure: FnPtr| {
                let mut def = this.0.lock();
                def.install = Some(InstallDef {
                    closure,
                    requirements: BTreeMap::new(),
                });
            })
            .with_fn(
                "on_install",
                |this: &mut Self, closure: FnPtr, requirements: Map| {
                    let reqs = parse_install_requirements(&requirements);
                    let mut def = this.0.lock();
                    def.install = Some(InstallDef {
                        closure,
                        requirements: reqs,
                    });
                },
            );

        // Ingress finalization: when service.ingress(hostname, port) creates an
        // IngressBuilder, the ingress needs to be registered. We provide this as
        // a method on IngressBuilder that stores into the app. But since Rhai
        // chains methods on IngressBuilder (tls, http, etc.) we need IngressBuilder
        // to produce an Ingress that gets stored. We handle this by making the
        // IngressBuilder methods available on Ingress directly, and converting
        // IngressBuilder -> Ingress at the point of first builder method call.
        //
        // Actually, the cleaner approach: service.ingress() returns IngressBuilder,
        // and IngressBuilder's methods (tls, http, service, etc.) just work on it.
        // We convert IngressBuilder to Ingress and store it when needed.
    }
}

fn parse_install_requirements(map: &Map) -> BTreeMap<String, InstallRequirementDef> {
    let mut reqs = BTreeMap::new();
    for (key, value) in map {
        if let Some(req_map) = value.read_lock::<Map>() {
            let kind = req_map
                .get("kind")
                .and_then(|v| v.clone().into_string().ok())
                .and_then(|s| InstallRequirementKind::from_str(&s))
                .unwrap_or_default();

            let required = req_map
                .get("required")
                .and_then(|v| v.as_bool().ok())
                .unwrap_or(true);

            let default_value = req_map
                .get("default_value")
                .and_then(|v| v.clone().into_string().ok());

            let description = req_map
                .get("description")
                .and_then(|v| v.clone().into_string().ok());

            reqs.insert(
                key.to_string(),
                InstallRequirementDef {
                    kind,
                    required,
                    default_value,
                    description,
                },
            );
        }
    }
    reqs
}
