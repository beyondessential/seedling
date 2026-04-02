use std::collections::BTreeMap;
use std::rc::Rc;

use rhai::{CustomType, Dynamic, EvalAltResult, FnPtr, Map, TypeBuilder};

use super::{
    Holder,
    action::{Action, ActionDef, ShellDef},
    collection::{AppBag, Collection},
    deployment::Deployment,
    install::{InstallDef, InstallRequirementDef, InstallRequirementKind},
    job::Job,
    resource::{Resource, ResourceId, ResourceKind, ResourceName},
    service::{ExternalService, Service},
    volume::{ExternalVolume, Volume},
};

// l[impl app.type]
// l[impl app.constructor]
#[derive(Debug, Default, Clone)]
pub struct App(pub Holder<AppDef>);

// l[impl app.resources]
// l[impl app.resources.names]
#[derive(Debug, Default, Clone)]
pub struct AppDef {
    pub name: String,
    pub params: BTreeMap<String, String>,
    pub resources: BTreeMap<ResourceId, Resource>,
    pub actions: BTreeMap<String, ActionDef>,
    pub shells: BTreeMap<String, ShellDef>,
    pub install: Option<InstallDef>,
    pub param_changes: BTreeMap<String, FnPtr>,
}

fn extract_description(options: &Map) -> Option<String> {
    options
        .get("description")
        .and_then(|v| v.clone().into_string().ok())
}

// l[impl app.methods]
impl CustomType for App {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("App");

        // l[impl param.type]
        builder.with_fn(
            "param",
            |this: &mut Self, name: &str| -> super::param::Param {
                let mut def = this.0.lock();
                let value = def
                    .params
                    .entry(name.into())
                    // l[impl bsl.placeholder]
                    .or_insert_with(|| "<placeholder>".into())
                    .clone();
                super::param::Param {
                    name: name.into(),
                    value,
                    app: this.clone(),
                }
            },
        );

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

        // l[impl collection.select]
        builder.with_fn("select", |this: &mut Self, criterion: Map| -> Collection {
            Collection::from_bag(Rc::new(AppBag(this.clone()))).select(&criterion)
        });

        // l[impl collection.one]
        builder.with_fn("one", |this: &mut Self| -> Dynamic {
            Collection::from_bag(Rc::new(AppBag(this.clone()))).one()
        });

        // l[impl collection.only]
        builder.with_fn("only", |this: &mut Self, other: Dynamic| -> Collection {
            Collection::from_bag(Rc::new(AppBag(this.clone()))).only(other)
        });

        // l[impl collection.except]
        builder.with_fn("except", |this: &mut Self, other: Dynamic| -> Collection {
            Collection::from_bag(Rc::new(AppBag(this.clone()))).except(other)
        });

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
                |this: &mut Self,
                 closure: FnPtr,
                 requirements: Map|
                 -> Result<(), Box<EvalAltResult>> {
                    let reqs = parse_install_requirements(&requirements)?;
                    let mut def = this.0.lock();
                    def.install = Some(InstallDef {
                        closure,
                        requirements: reqs,
                    });
                    Ok(())
                },
            );
    }
}

// l[impl action.install.requirements.kind-unknown]
fn parse_install_requirements(
    map: &Map,
) -> Result<BTreeMap<String, InstallRequirementDef>, Box<EvalAltResult>> {
    let mut reqs = BTreeMap::new();
    for (key, value) in map {
        if let Some(req_map) = value.read_lock::<Map>() {
            let kind = match req_map
                .get("kind")
                .and_then(|v| v.clone().into_string().ok())
            {
                Some(s) => InstallRequirementKind::from_str(&s).ok_or_else(|| {
                    Box::<EvalAltResult>::from(format!("unknown install requirement kind: \"{s}\""))
                })?,
                None => InstallRequirementKind::default(),
            };

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
    Ok(reqs)
}
