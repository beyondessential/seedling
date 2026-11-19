use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use rhai::{CustomType, Dynamic, EvalAltResult, FnPtr, Map, TypeBuilder};
use seedling_protocol::names::GateName;
use serde_json::Value;

use super::{
    app::{App, AppDef, capture_gate, ensure_not_in_action_closure, ensure_not_validating},
    collection::{Collection, Member, col},
    deployment::Deployment,
    job::Job,
    resource::{Resource, ResourceId, ResourceKind},
    take::{reject_unknown_keys, take_closure, take_number},
};
use crate::runtime::secrets::{Cipher, Error as SecretsError, KeyDigest};

// spec: GATE#keys
/// The value a gate's definition gives it, held as the JSON value it is the
/// same as another key by.
#[derive(Debug, Clone)]
pub struct GateKey(Value);

impl Default for GateKey {
    fn default() -> Self {
        Self(Value::Null)
    }
}

impl PartialEq for GateKey {
    fn eq(&self, other: &Self) -> bool {
        self.canonical_bytes() == other.canonical_bytes()
    }
}

impl Eq for GateKey {}

impl GateKey {
    /// The key a script's value stands for, or a throw naming where the value
    /// is not representable in JSON.
    // spec: GATE#keys
    pub fn from_script(value: Dynamic) -> Result<Self, Box<EvalAltResult>> {
        json_of("`key`", value).map(Self)
    }

    pub fn as_json(&self) -> &Value {
        &self.0
    }

    /// The bytes the key is told apart from another by: its JSON text, with
    /// the members of every map in order of name, so that two keys are the
    /// same exactly when these are.
    // spec: GATE#keys
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = String::new();
        write_canonical(&self.0, &mut out);
        out.into_bytes()
    }

    pub fn digest(&self, cipher: &Cipher) -> Result<KeyDigest, SecretsError> {
        cipher.key_digest(&self.canonical_bytes())
    }
}

// spec: GATE#keys
fn json_of(path: &str, value: Dynamic) -> Result<Value, Box<EvalAltResult>> {
    let value = value.flatten();
    // take: dispatch
    if value.is_unit() {
        return Ok(Value::Null);
    }
    // take: dispatch
    if let Ok(b) = value.as_bool() {
        return Ok(Value::Bool(b));
    }
    // take: dispatch
    if let Ok(n) = value.as_int() {
        return Ok(Value::from(n));
    }
    // take: dispatch
    if let Ok(f) = value.as_float() {
        // Negative zero is the same JSON value as zero.
        let f = if f == 0.0 { 0.0 } else { f };
        return serde_json::Number::from_f64(f)
            .map(Value::Number)
            .ok_or_else(|| {
                format!("{path} must be representable in JSON, but {f} is not a finite number")
                    .into()
            });
    }
    // take: dispatch
    if let Ok(s) = value.clone().into_string() {
        return Ok(Value::String(s));
    }
    // take: dispatch
    if let Some(array) = value.clone().try_cast::<rhai::Array>() {
        return array
            .into_iter()
            .enumerate()
            .map(|(index, item)| json_of(&format!("{path}[{index}]"), item))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array);
    }
    // take: dispatch
    if let Some(map) = value.clone().try_cast::<Map>() {
        return map
            .into_iter()
            .map(|(name, item)| {
                json_of(&format!("{path}.{name}"), item).map(|v| (name.to_string(), v))
            })
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(Value::Object);
    }
    Err(format!(
        "{path} must be representable in JSON: a unit, boolean, integer, finite number, string, \
         or an array or map of these, got {}",
        value.type_name()
    )
    .into())
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(members) => {
            let mut sorted: Vec<_> = members.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (index, (name, member)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(name.clone()).to_string());
                out.push(':');
                write_canonical(member, out);
            }
            out.push('}');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// What a gate or workload is ordered after.
// spec: GATE#start-order
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum AfterTarget {
    Gate(GateName),
    Workload(ResourceId),
}

impl AfterTarget {
    fn describe(&self) -> String {
        match self {
            Self::Gate(name) => format!("gate '{name}'"),
            Self::Workload(id) => format!("{} '{}'", id.kind.type_name(), id.name),
        }
    }
}

/// The check a gate declares.
// spec: GATE#check
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateCheck {
    pub job: ResourceId,
    pub interval: Option<Duration>,
}

/// What a script declares of a gate, as the runtime reads it. The body is a
/// closure, so it stays with the evaluation that made it and is picked from
/// there when the gate runs.
// spec: GATE#declaration
#[derive(Debug, Clone, PartialEq)]
pub struct GateDef {
    /// Where the script declared the gate among its gates, so that gates due
    /// together run in the order the script wrote them.
    pub declaration: usize,
    pub key: GateKey,
    pub has_body: bool,
    pub check: Option<GateCheck>,
    /// The Deployments and Jobs the gate holds stopped for its run, resolved
    /// once the script's evaluation completes.
    pub exclusive: BTreeSet<ResourceId>,
    pub after: BTreeSet<AfterTarget>,
}

/// The digests of the keys of an app's gates under the host's secret key, as
/// a workload's spec hash carries them.
// spec: UPD#spec-hash
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GateKeyDigests(BTreeMap<GateName, KeyDigest>);

/// A workload is ordered after a gate that the digests hold nothing for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingGateKey(pub GateName);

impl std::fmt::Display for MissingGateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no key digest for gate '{}'", self.0)
    }
}

impl std::error::Error for MissingGateKey {}

impl GateKeyDigests {
    pub fn of(def: &AppDef, cipher: &Cipher) -> Result<Self, SecretsError> {
        def.gates
            .iter()
            .map(|(name, gate)| Ok((name.clone(), gate.key.digest(cipher)?)))
            .collect::<Result<_, SecretsError>>()
            .map(Self)
    }

    /// The digest of the key of gate `name`, or `None` for a gate the
    /// definition does not declare.
    pub fn get(&self, name: &GateName) -> Option<KeyDigest> {
        self.0.get(name).copied()
    }

    #[cfg(test)]
    pub fn from_digests(digests: impl IntoIterator<Item = (GateName, KeyDigest)>) -> Self {
        Self(digests.into_iter().collect())
    }

    /// The digests of the gates `resource` is ordered directly after, in order
    /// of gate name. A gate ordered after through another target is not
    /// among them.
    // spec: UPD#spec-hash
    pub fn directly_after(
        &self,
        resource: &Resource,
    ) -> Result<Vec<(GateName, KeyDigest)>, MissingGateKey> {
        let Some(after) = workload_after(resource) else {
            return Ok(Vec::new());
        };
        after
            .iter()
            .filter_map(|target| match target {
                AfterTarget::Gate(name) => Some(name),
                AfterTarget::Workload(_) => None,
            })
            .map(|name| {
                self.0
                    .get(name)
                    .map(|digest| (name.clone(), *digest))
                    .ok_or_else(|| MissingGateKey(name.clone()))
            })
            .collect()
    }
}

/// What a Deployment or Job is ordered after, or `None` for any other
/// resource.
pub fn workload_after(resource: &Resource) -> Option<BTreeSet<AfterTarget>> {
    match resource {
        Resource::Deployment(d) => Some(d.def.lock().pod.lock().after.clone()),
        Resource::Job(j) => Some(j.def.lock().pod.lock().after.clone()),
        _ => None,
    }
}

/// The targets named by `target`: a gate, a named Deployment, a named Job, or
/// an array of these.
// spec: GATE#start-order
pub fn after_targets(target: Dynamic) -> Result<Vec<AfterTarget>, Box<EvalAltResult>> {
    fn one(what: &str, value: Dynamic) -> Result<AfterTarget, Box<EvalAltResult>> {
        let value = value.flatten();
        // take: dispatch
        if let Some(gate) = value.clone().try_cast::<Gate>() {
            return Ok(AfterTarget::Gate(gate.name));
        }
        // take: dispatch
        if let Some(dep) = value.clone().try_cast::<Deployment>() {
            return named(
                what,
                ResourceKind::Deployment,
                &dep.name,
                dep.anon_id.is_some(),
            );
        }
        // take: dispatch
        if let Some(job) = value.clone().try_cast::<Job>() {
            return named(what, ResourceKind::Job, &job.name, job.anon_id.is_some());
        }
        Err(format!(
            "{what} must be a gate, a named deployment, a named job, or an array of these, got {}",
            value.type_name()
        )
        .into())
    }

    fn named(
        what: &str,
        kind: ResourceKind,
        name: &super::resource::ResourceName,
        anonymous: bool,
    ) -> Result<AfterTarget, Box<EvalAltResult>> {
        if anonymous {
            return Err(format!(
                "{what} must be named: an anonymous {} cannot be waited on",
                kind.type_name()
            )
            .into());
        }
        Ok(AfterTarget::Workload(ResourceId {
            kind,
            name: name.clone(),
        }))
    }

    let target = target.flatten();
    // take: dispatch
    if let Some(array) = target.clone().try_cast::<rhai::Array>() {
        return array
            .into_iter()
            .enumerate()
            .map(|(index, item)| one(&format!("`target` element {index}"), item))
            .collect();
    }
    one("`target`", target).map(|t| vec![t])
}

/// Refuse a call made outside the static context.
fn ensure_static(method: &str) -> Result<(), Box<EvalAltResult>> {
    ensure_not_validating()?;
    ensure_not_in_action_closure(method)
}

/// Refuse a selection that is not one a runtime handle method takes.
// spec: GATE#exclusive-selection
fn ensure_selection(what: &str, value: &Dynamic) -> Result<(), Box<EvalAltResult>> {
    let is_selection = value.is::<Collection>()
        || value.is::<App>()
        || value.is::<Deployment>()
        || value.is::<Job>()
        || value.is::<super::service::Service>()
        || value.is::<super::service::HttpService>()
        || value.is::<super::service::ExternalService>()
        || value.is::<super::ingress::Ingress>()
        || value.is::<super::volume::Volume>()
        || value.is::<super::volume::ExternalVolume>();
    if is_selection {
        return Ok(());
    }
    // take: dispatch
    if let Some(array) = value.clone().try_cast::<rhai::Array>() {
        return array
            .iter()
            .try_for_each(|element| ensure_selection(what, element));
    }
    if value.is::<Gate>() {
        return Err(format!("{what} must be a selection: a gate is not a resource").into());
    }
    Err(format!(
        "{what} must be an app, a collection, a resource or an array of these, got {}",
        value.type_name()
    )
    .into())
}

// spec: GATE#declaration
#[derive(Debug, Clone)]
pub struct Gate {
    pub name: GateName,
    app: App,
}

impl Gate {
    fn new(name: GateName, app: App) -> Self {
        Self { name, app }
    }

    /// Apply `change` to the gate this handle names, which its app declares.
    fn edit(&self, change: impl Fn(&mut GateDef)) -> Result<(), Box<EvalAltResult>> {
        let mut declared = true;
        self.app.def.rcu(|d| {
            let mut d = (**d).clone();
            match d.gates.get_mut(&self.name) {
                Some(gate) => {
                    declared = true;
                    change(gate);
                }
                None => declared = false,
            }
            d
        });
        if declared {
            Ok(())
        } else {
            Err(format!("gate '{}' is not declared by its app", self.name).into())
        }
    }
}

impl CustomType for Gate {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("Gate");

        // spec: GATE#keys
        builder.with_fn(
            "key",
            |this: &mut Self, value: Dynamic| -> Result<Self, Box<EvalAltResult>> {
                ensure_static("key")?;
                let key = GateKey::from_script(value)?;
                this.edit(|gate| gate.key = key.clone())?;
                Ok(this.clone())
            },
        );

        // spec: GATE#body
        builder.with_fn(
            "run",
            |this: &mut Self, body: FnPtr| -> Result<Self, Box<EvalAltResult>> {
                // spec: BSL#arguments
                let body = take_closure("run", "fn", body)?;
                ensure_static("run")?;
                if this
                    .app
                    .def
                    .load()
                    .gates
                    .get(&this.name)
                    .is_some_and(|g| g.has_body)
                {
                    return Err(format!("gate '{}' already has a body", this.name).into());
                }
                this.edit(|gate| gate.has_body = true)?;
                capture_gate(this.name.clone(), Some(body));
                Ok(this.clone())
            },
        );

        // spec: GATE#check
        builder
            .with_fn(
                "check",
                |this: &mut Self, job: Job| -> Result<Self, Box<EvalAltResult>> {
                    this.set_check(&job, None)
                },
            )
            .with_fn(
                "check",
                |this: &mut Self, job: Job, options: Map| -> Result<Self, Box<EvalAltResult>> {
                    reject_unknown_keys("check options", &options, &["interval"])?;
                    let interval = options
                        .get("interval")
                        .cloned()
                        .map(|v| {
                            let seconds = take_number("interval", v)?;
                            if seconds < 1.0 {
                                return Err(format!(
                                    "`interval` must be at least 1 second, got {seconds}"
                                )
                                .into());
                            }
                            Duration::try_from_secs_f64(seconds).map_err(
                                |e| -> Box<EvalAltResult> {
                                    format!("`interval` is out of range: {e}").into()
                                },
                            )
                        })
                        .transpose()?;
                    this.set_check(&job, interval)
                },
            );

        // spec: GATE#exclusive-selection
        builder.with_fn(
            "exclusive",
            |this: &mut Self, selection: Dynamic| -> Result<Self, Box<EvalAltResult>> {
                ensure_static("exclusive")?;
                ensure_selection("`selection`", &selection)?;
                let collection = col(selection);
                for handle in collection.resolve() {
                    if let Member::Anonymous { kind, .. } = handle.member() {
                        return Err(format!(
                            "`selection` holds an anonymous {}, which is a dynamic resource a \
                             gate cannot hold",
                            kind.type_name()
                        )
                        .into());
                    }
                }
                this.app.hold_gate_selection(this.name.clone(), collection);
                Ok(this.clone())
            },
        );

        // spec: GATE#start-order
        builder.with_fn(
            "after",
            |this: &mut Self, target: Dynamic| -> Result<Self, Box<EvalAltResult>> {
                ensure_static("after")?;
                let targets = after_targets(target)?;
                this.edit(|gate| gate.after.extend(targets.iter().cloned()))?;
                Ok(this.clone())
            },
        );
    }
}

impl Gate {
    // spec: GATE#check
    fn set_check(&self, job: &Job, interval: Option<Duration>) -> Result<Self, Box<EvalAltResult>> {
        ensure_static("check")?;
        if job.anon_id.is_some() {
            return Err("`check` must be given a named job, not an anonymous one".into());
        }
        if job.def.lock().deadline.is_none() {
            return Err(format!(
                "job '{}' has no deadline, and a check's job needs one so that a check that \
                 never ends fails; call .deadline(...) on it before it is made a check",
                job.name
            )
            .into());
        }
        let id = ResourceId {
            kind: ResourceKind::Job,
            name: job.name.clone(),
        };
        let def = self.app.def.load();
        if let Some(other) = def
            .gates
            .iter()
            .find(|(name, gate)| {
                **name != self.name && gate.check.as_ref().is_some_and(|c| c.job == id)
            })
            .map(|(name, _)| name)
        {
            return Err(format!(
                "job '{}' is already the check of gate '{other}', and a job checks one gate",
                job.name
            )
            .into());
        }
        drop(def);
        self.edit(|gate| {
            gate.check = Some(GateCheck {
                job: id.clone(),
                interval,
            })
        })?;
        Ok(self.clone())
    }
}

/// Declare gate `name` in `app`, or find the one declared, and return its
/// handle.
// spec: GATE#declaration
pub(crate) fn declare(app: &App, name: GateName) -> Gate {
    app.def.rcu(|d| {
        let mut d = (**d).clone();
        let declaration = d.gates.len();
        d.gates.entry(name.clone()).or_insert_with(|| GateDef {
            declaration,
            key: GateKey::default(),
            has_body: false,
            check: None,
            exclusive: BTreeSet::new(),
            after: BTreeSet::new(),
        });
        d
    });
    capture_gate(name.clone(), None);
    Gate::new(name, app.clone())
}

/// Resolve the exclusive selections the script gave its gates, now that
/// every resource it declares is there to be selected.
// spec: GATE#exclusive-selection
pub(crate) fn resolve_exclusive_selections(app: &App) -> Result<(), String> {
    let selections = app.take_gate_selections();
    let mut resolved: BTreeMap<GateName, BTreeSet<ResourceId>> = BTreeMap::new();
    for (name, collection) in selections {
        let mut ids = BTreeSet::new();
        for handle in collection.resolve() {
            match handle.member() {
                Member::Named(id)
                    if matches!(id.kind, ResourceKind::Deployment | ResourceKind::Job) =>
                {
                    ids.insert(id.clone());
                }
                Member::Named(_) => {}
                Member::Anonymous { kind, .. } => {
                    return Err(format!(
                        "gate '{name}' holds an anonymous {} in its exclusive selection",
                        kind.type_name()
                    ));
                }
            }
        }
        resolved.insert(name, ids);
    }
    app.def.rcu(|d| {
        let mut d = (**d).clone();
        for (name, ids) in &resolved {
            if let Some(gate) = d.gates.get_mut(name) {
                gate.exclusive = ids.clone();
            }
        }
        d
    });
    Ok(())
}

type Graph = BTreeMap<AfterTarget, BTreeSet<AfterTarget>>;

fn ordering(def: &AppDef) -> Graph {
    let gates = def
        .gates
        .iter()
        .map(|(name, gate)| (AfterTarget::Gate(name.clone()), gate.after.clone()));
    let workloads = def.resources.iter().filter_map(|(id, resource)| {
        workload_after(resource).map(|after| (AfterTarget::Workload(id.clone()), after))
    });
    gates.chain(workloads).collect()
}

fn find_cycle(
    node: &AfterTarget,
    graph: &Graph,
    path: &mut Vec<AfterTarget>,
    done: &mut BTreeSet<AfterTarget>,
) -> Option<Vec<AfterTarget>> {
    if let Some(at) = path.iter().position(|n| n == node) {
        let mut cycle = path[at..].to_vec();
        cycle.push(node.clone());
        return Some(cycle);
    }
    if done.contains(node) {
        return None;
    }
    path.push(node.clone());
    for target in graph.get(node).into_iter().flatten() {
        if let Some(cycle) = find_cycle(target, graph, path, done) {
            return Some(cycle);
        }
    }
    path.pop();
    done.insert(node.clone());
    None
}

/// Everything `start` is ordered after, directly or through other targets.
fn waits_on(start: &AfterTarget, graph: &Graph) -> BTreeSet<AfterTarget> {
    let mut seen = BTreeSet::new();
    let mut pending: Vec<&AfterTarget> = graph.get(start).into_iter().flatten().collect();
    while let Some(next) = pending.pop() {
        if seen.insert(next.clone()) {
            pending.extend(graph.get(next).into_iter().flatten());
        }
    }
    seen
}

/// Check the rules the gates of a definition are held to once it has
/// evaluated in full, which no single call could check because a later call
/// can change what an earlier one depends on.
// spec: GATE#start-order
// spec: GATE#exclusive-selection
// spec: GATE#check
pub(crate) fn check_gates(def: &AppDef) -> Result<(), String> {
    let graph = ordering(def);

    for (node, targets) in &graph {
        for target in targets {
            let declared = match target {
                AfterTarget::Gate(name) => def.gates.contains_key(name),
                AfterTarget::Workload(id) => def.resources.contains_key(id),
            };
            if !declared {
                return Err(format!(
                    "{} is ordered after {}, which this app does not declare",
                    node.describe(),
                    target.describe()
                ));
            }
        }
    }

    let mut done = BTreeSet::new();
    for node in graph.keys() {
        if let Some(cycle) = find_cycle(node, &graph, &mut Vec::new(), &mut done) {
            return Err(if cycle.len() == 2 {
                format!("{} cannot be ordered after itself", cycle[0].describe())
            } else {
                let names: Vec<_> = cycle.iter().map(AfterTarget::describe).collect();
                format!("ordering forms a cycle: {}", names.join(" -> "))
            });
        }
    }

    for (name, gate) in &def.gates {
        let this = AfterTarget::Gate(name.clone());
        let waits = waits_on(&this, &graph);
        if let Some(held) = gate
            .exclusive
            .iter()
            .find(|id| waits.contains(&AfterTarget::Workload((*id).clone())))
        {
            return Err(format!(
                "gate '{name}' holds {} '{}' in its exclusive selection, which it is ordered \
                 after",
                held.kind.type_name(),
                held.name
            ));
        }
        if let Some(check) = &gate.check {
            if gate.exclusive.contains(&check.job) {
                return Err(format!(
                    "the check of gate '{name}', job '{}', is in its exclusive selection",
                    check.job.name
                ));
            }
            if waits_on(&AfterTarget::Workload(check.job.clone()), &graph).contains(&this) {
                return Err(format!(
                    "the check of gate '{name}', job '{}', is ordered after the gate",
                    check.job.name
                ));
            }
        }
    }
    Ok(())
}
