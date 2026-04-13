use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::str::FromStr as _;

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
use crate::runtime::barrier::runtime::{action_def, is_in_action_closure};

// ---------------------------------------------------------------------------
// Thread-local closure capture buffer
// ---------------------------------------------------------------------------

/// Closures registered by the BSL script during a single re-run in
/// `run_operation`. Never stored persistently — activated on demand, consumed
/// immediately after the re-run, then discarded.
#[derive(Default)]
pub(crate) struct ClosureCapture {
    pub actions: BTreeMap<String, FnPtr>,
    pub shells: BTreeMap<String, FnPtr>,
    pub install: Option<FnPtr>,
    pub param_changes: BTreeMap<String, FnPtr>,
}

thread_local! {
    static CLOSURE_CAPTURE: RefCell<Option<ClosureCapture>> = const { RefCell::new(None) };
}

/// Activate the closure capture buffer on this thread. While active, every
/// `on_start`, `on_action`, `on_shell`, `on_install`, and `param.on_change`
/// call will push its `FnPtr` into the buffer in addition to writing metadata
/// into `AppDef`. Has no effect (and causes no allocation) when not active.
pub(crate) fn begin_closure_capture() {
    CLOSURE_CAPTURE.with(|c| *c.borrow_mut() = Some(ClosureCapture::default()));
}

/// Deactivate the buffer and return whatever was captured. Must be called
/// exactly once after `begin_closure_capture`, even if the script run fails.
pub(crate) fn end_closure_capture() -> ClosureCapture {
    CLOSURE_CAPTURE.with(|c| c.borrow_mut().take().unwrap_or_default())
}

/// Called by `param.on_change` — writes the FnPtr into the active buffer if
/// one exists, otherwise silently discards it.
pub(crate) fn capture_param_change(name: String, fnptr: FnPtr) {
    CLOSURE_CAPTURE.with(|c| {
        if let Some(ref mut store) = *c.borrow_mut() {
            store.param_changes.insert(name, fnptr);
        }
    });
}

fn capture_action(name: String, fnptr: FnPtr) {
    CLOSURE_CAPTURE.with(|c| {
        if let Some(ref mut store) = *c.borrow_mut() {
            store.actions.insert(name, fnptr);
        }
    });
}

fn capture_shell(name: String, fnptr: FnPtr) {
    CLOSURE_CAPTURE.with(|c| {
        if let Some(ref mut store) = *c.borrow_mut() {
            store.shells.insert(name, fnptr);
        }
    });
}

fn capture_install(fnptr: FnPtr) {
    CLOSURE_CAPTURE.with(|c| {
        if let Some(ref mut store) = *c.borrow_mut() {
            store.install = Some(fnptr);
        }
    });
}

// ---------------------------------------------------------------------------
// AppDef — Send, shared with the Reconciler
// ---------------------------------------------------------------------------

// l[impl app.resources]
// l[impl app.resources.names]
#[derive(Debug, Default, Clone)]
pub struct AppDef {
    pub name: String,
    /// Names of parameters declared by the BSL script via `app.param()`.
    pub params: BTreeSet<String>,
    pub resources: BTreeMap<ResourceId, Resource>,
    /// Action metadata (name, description). No FnPtrs — closures are
    /// recovered on demand via the thread-local capture buffer.
    pub actions: BTreeMap<String, ActionDef>,
    pub shells: BTreeMap<String, ShellDef>,
    pub install: Option<InstallDef>,
    /// Names of parameters that have an `on_change` handler registered.
    pub param_changes: BTreeSet<String>,
}

fn extract_description(options: &Map) -> Option<String> {
    options
        .get("description")
        .and_then(|v| v.clone().into_string().ok())
}

// ---------------------------------------------------------------------------
// App — the BSL-facing handle; !Send (Rc inside), stays on the BSL thread
// ---------------------------------------------------------------------------

// l[impl app.type]
// l[impl app.constructor]
#[derive(Clone, Default)]
pub struct App {
    pub def: Holder<AppDef>,
    /// Operator-provided parameter values, pre-populated from the database before
    /// script evaluation. Not BSL-driven — the script cannot modify this directly.
    pub stored: Holder<BTreeMap<String, String>>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("def", &self.def)
            .field("stored", &self.stored)
            .finish_non_exhaustive()
    }
}

// l[impl app.methods]
impl CustomType for App {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("App");

        // l[impl param.type]
        builder.with_fn(
            "param",
            |this: &mut Self, name: &str| -> super::param::Param {
                let value = this.stored.lock().get(name).cloned();
                this.def.lock().params.insert(name.into());
                super::param::Param {
                    name: name.into(),
                    value,
                    app: this.clone(),
                }
            },
        );

        // l[impl service.type]
        // l[impl app.resources.context.named]
        builder.with_fn(
            "service",
            |this: &mut Self, name: &str| -> Result<Service, Box<EvalAltResult>> {
                let rname = ResourceName::new(name.into());
                if is_in_action_closure() {
                    let adef = action_def().ok_or_else(|| -> Box<EvalAltResult> {
                        "internal: action context but no action AppDef set".into()
                    })?;
                    let def = adef.lock();
                    let id = ResourceId {
                        kind: ResourceKind::Service,
                        name: rname,
                    };
                    match def.resources.get(&id) {
                        Some(Resource::Service(s)) => {
                            let mut frozen = s.clone();
                            frozen.frozen = true;
                            Ok(frozen)
                        }
                        Some(_) => Err(format!("'{}' is not a service", name).into()),
                        None => Err(format!("no static service named '{}'", name).into()),
                    }
                } else {
                    let weak = std::sync::Arc::downgrade(&this.def);
                    let mut def = this.def.lock();
                    let id = ResourceId {
                        kind: ResourceKind::Service,
                        name: rname.clone(),
                    };
                    let resource = def.resources.entry(id).or_insert_with(|| {
                        Resource::Service(Service::new_with_app(rname, weak.clone()))
                    });
                    match resource {
                        Resource::Service(s) => {
                            if s.app_def.is_none() {
                                s.app_def = Some(weak);
                            }
                            Ok(s.clone())
                        }
                        _ => unreachable!(),
                    }
                }
            },
        );

        // l[impl app.resources.context.anonymous]
        builder.with_fn(
            "service",
            |_this: &mut Self| -> Result<Service, Box<EvalAltResult>> {
                if !is_in_action_closure() {
                    return Err(
                        "anonymous services can only be created inside action closures".into(),
                    );
                }
                Ok(Service::new(ResourceName::new(String::new())))
            },
        );

        // l[impl service.external]
        builder.with_fn(
            "external_service",
            |this: &mut Self, name: &str| -> Dynamic {
                let rname = ResourceName::new(name.into());
                let mut def = this.def.lock();
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
        // l[impl app.resources.context.named]
        builder.with_fn(
            "deployment",
            |this: &mut Self, name: &str| -> Result<Deployment, Box<EvalAltResult>> {
                let rname = ResourceName::new(name.into());
                if is_in_action_closure() {
                    let adef = action_def().ok_or_else(|| -> Box<EvalAltResult> {
                        "internal: action context but no action AppDef set".into()
                    })?;
                    let def = adef.lock();
                    let id = ResourceId {
                        kind: ResourceKind::Deployment,
                        name: rname,
                    };
                    match def.resources.get(&id) {
                        Some(Resource::Deployment(d)) => {
                            let mut frozen = d.clone();
                            frozen.frozen = true;
                            Ok(frozen)
                        }
                        Some(_) => Err(format!("'{}' is not a deployment", name).into()),
                        None => Err(format!("no static deployment named '{}'", name).into()),
                    }
                } else {
                    let mut def = this.def.lock();
                    let id = ResourceId {
                        kind: ResourceKind::Deployment,
                        name: rname.clone(),
                    };
                    let resource = def.resources.entry(id).or_insert_with(|| {
                        Resource::Deployment(Deployment {
                            name: rname,
                            def: Default::default(),
                            frozen: false,
                        })
                    });
                    match resource {
                        Resource::Deployment(d) => Ok(d.clone()),
                        _ => unreachable!(),
                    }
                }
            },
        );

        // l[impl app.resources.context.anonymous]
        builder.with_fn(
            "deployment",
            |_this: &mut Self| -> Result<Deployment, Box<EvalAltResult>> {
                if !is_in_action_closure() {
                    return Err(
                        "anonymous deployments can only be created inside action closures".into(),
                    );
                }
                Ok(Deployment {
                    name: ResourceName::new(String::new()),
                    def: Default::default(),
                    frozen: false,
                })
            },
        );

        // l[impl job.type]
        // l[impl app.resources.context.named]
        builder.with_fn(
            "job",
            |this: &mut Self, name: &str| -> Result<Job, Box<EvalAltResult>> {
                let rname = ResourceName::new(name.into());
                if is_in_action_closure() {
                    let adef = action_def().ok_or_else(|| -> Box<EvalAltResult> {
                        "internal: action context but no action AppDef set".into()
                    })?;
                    let def = adef.lock();
                    let id = ResourceId {
                        kind: ResourceKind::Job,
                        name: rname,
                    };
                    match def.resources.get(&id) {
                        Some(Resource::Job(j)) => {
                            let mut frozen = j.clone();
                            frozen.frozen = true;
                            Ok(frozen)
                        }
                        Some(_) => Err(format!("'{}' is not a job", name).into()),
                        None => Err(format!("no static job named '{}'", name).into()),
                    }
                } else {
                    let mut def = this.def.lock();
                    let id = ResourceId {
                        kind: ResourceKind::Job,
                        name: rname.clone(),
                    };
                    let resource = def.resources.entry(id).or_insert_with(|| {
                        Resource::Job(Job {
                            name: rname,
                            def: Default::default(),
                            frozen: false,
                        })
                    });
                    match resource {
                        Resource::Job(j) => Ok(j.clone()),
                        _ => unreachable!(),
                    }
                }
            },
        );

        // l[impl app.resources.context.anonymous]
        builder.with_fn(
            "job",
            |_this: &mut Self| -> Result<Job, Box<EvalAltResult>> {
                if !is_in_action_closure() {
                    return Err("anonymous jobs can only be created inside action closures".into());
                }
                Ok(Job {
                    name: ResourceName::new(String::new()),
                    def: Default::default(),
                    frozen: false,
                })
            },
        );

        // l[impl volume.type]
        // l[impl app.resources.context.named]
        builder.with_fn(
            "volume",
            |this: &mut Self, name: &str| -> Result<Volume, Box<EvalAltResult>> {
                let rname = ResourceName::new(name.into());
                if is_in_action_closure() {
                    let adef = action_def().ok_or_else(|| -> Box<EvalAltResult> {
                        "internal: action context but no action AppDef set".into()
                    })?;
                    let def = adef.lock();
                    let id = ResourceId {
                        kind: ResourceKind::Volume,
                        name: rname,
                    };
                    match def.resources.get(&id) {
                        Some(Resource::Volume(v)) => {
                            let mut frozen = v.clone();
                            frozen.frozen = true;
                            Ok(frozen)
                        }
                        Some(_) => Err(format!("'{}' is not a volume", name).into()),
                        None => Err(format!("no static volume named '{}'", name).into()),
                    }
                } else {
                    let mut def = this.def.lock();
                    let id = ResourceId {
                        kind: ResourceKind::Volume,
                        name: rname.clone(),
                    };
                    let resource = def
                        .resources
                        .entry(id)
                        .or_insert_with(|| Resource::Volume(Volume::new(Some(rname))));
                    match resource {
                        Resource::Volume(v) => Ok(v.clone()),
                        _ => unreachable!(),
                    }
                }
            },
        );

        // l[impl volume.type]
        // l[impl app.resources.context.anonymous]
        builder.with_fn(
            "volume",
            |_this: &mut Self| -> Result<Volume, Box<EvalAltResult>> {
                if !is_in_action_closure() {
                    return Err(
                        "anonymous volumes can only be created inside action closures".into(),
                    );
                }
                let anon_id =
                    crate::runtime::barrier::runtime::next_anon_vol_id().unwrap_or_else(|| {
                        format!("seedling-anon-fallback-vol-{}", uuid::Uuid::new_v4())
                    });
                Ok(Volume::new_anonymous(anon_id))
            },
        );

        // l[impl volume.external]
        builder.with_fn(
            "external_volume",
            |this: &mut Self, name: &str| -> Dynamic {
                let rname = ResourceName::new(name.into());
                let mut def = this.def.lock();
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
                    this.def.lock().actions.insert(
                        name.into(),
                        ActionDef {
                            name: name.into(),
                            description: None,
                        },
                    );
                    capture_action(name.into(), closure);
                    Action { name: name.into() }
                },
            )
            .with_fn(
                "on_action",
                |this: &mut Self, name: &str, closure: FnPtr, options: Map| -> Action {
                    let desc = extract_description(&options);
                    this.def.lock().actions.insert(
                        name.into(),
                        ActionDef {
                            name: name.into(),
                            description: desc,
                        },
                    );
                    capture_action(name.into(), closure);
                    Action { name: name.into() }
                },
            );

        // l[impl action.start]
        builder
            .with_fn("on_start", |this: &mut Self, closure: FnPtr| -> Action {
                this.def.lock().actions.insert(
                    "start".into(),
                    ActionDef {
                        name: "start".into(),
                        description: None,
                    },
                );
                capture_action("start".into(), closure);
                Action {
                    name: "start".into(),
                }
            })
            .with_fn(
                "on_start",
                |this: &mut Self, closure: FnPtr, options: Map| -> Action {
                    let desc = extract_description(&options);
                    this.def.lock().actions.insert(
                        "start".into(),
                        ActionDef {
                            name: "start".into(),
                            description: desc,
                        },
                    );
                    capture_action("start".into(), closure);
                    Action {
                        name: "start".into(),
                    }
                },
            );

        // l[impl action.shell]
        builder
            .with_fn("on_shell", |this: &mut Self, name: &str, closure: FnPtr| {
                this.def.lock().shells.insert(
                    name.into(),
                    ShellDef {
                        name: name.into(),
                        description: None,
                    },
                );
                capture_shell(name.into(), closure);
            })
            .with_fn(
                "on_shell",
                |this: &mut Self, name: &str, closure: FnPtr, options: Map| {
                    let desc = extract_description(&options);
                    this.def.lock().shells.insert(
                        name.into(),
                        ShellDef {
                            name: name.into(),
                            description: desc,
                        },
                    );
                    capture_shell(name.into(), closure);
                },
            );

        // l[impl action.install]
        builder
            .with_fn("on_install", |this: &mut Self, closure: FnPtr| {
                this.def.lock().install = Some(InstallDef {
                    requirements: BTreeMap::new(),
                });
                capture_install(closure);
            })
            .with_fn(
                "on_install",
                |this: &mut Self,
                 closure: FnPtr,
                 requirements: Map|
                 -> Result<(), Box<EvalAltResult>> {
                    let reqs = parse_install_requirements(&requirements)?;
                    this.def.lock().install = Some(InstallDef { requirements: reqs });
                    capture_install(closure);
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
                Some(s) => InstallRequirementKind::from_str(&s).map_err(|_| {
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
