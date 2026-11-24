// BSL definition structs carry fields that the runtime will consume once the
// reconciliation loop is wired up. Until then, suppress dead_code warnings.
#![allow(dead_code)]

use std::sync::Arc;

use parking_lot::Mutex;
use rhai::{Engine, Scope};

use crate::runtime::barrier::{runtime, shell};

type Holder<T> = Arc<Mutex<T>>;

/// Trait for BSL resource types that can be returned as frozen references
/// in action context. Builder methods call `ensure_unfrozen()?` to prevent
/// modification of static resource references inside action closures, and
/// of every resource inside a validator.
pub trait Freezable {
    fn is_frozen(&self) -> bool;

    // spec: APP#validator-immutability
    fn ensure_unfrozen(&self) -> Result<(), Box<rhai::EvalAltResult>> {
        app::ensure_not_validating()?;
        if self.is_frozen() {
            Err("cannot modify a static resource reference inside an action closure".into())
        } else {
            Ok(())
        }
    }
}

// spec: BSL#names
pub fn validate_name(name: &str) -> Result<(), Box<rhai::EvalAltResult>> {
    if name.starts_with('_') {
        return Err("name must not start with an underscore".into());
    }
    let ok = name.len() >= 3
        && name.len() <= 63
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.ends_with(|c: char| c.is_ascii_alphanumeric())
        && name[1..name.len() - 1]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid resource name '{name}': must be 3–63 ASCII alphanumeric/hyphen characters, \
             start with a letter, and not start or end with a hyphen"
        )
        .into())
    }
}

// spec: BSL#ports
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Port(u16);

impl Port {
    pub fn new(raw: i64) -> Result<Self, Box<rhai::EvalAltResult>> {
        let port = u16::try_from(raw).map_err(|_| -> Box<rhai::EvalAltResult> {
            format!("port must be an integer between 1 and 65535, got {raw}").into()
        })?;
        if port == 0 {
            return Err("port must be an integer between 1 and 65535, got 0".into());
        }
        Ok(Self(port))
    }

    /// Construct from a known-valid literal. Panics in debug mode if `port == 0`.
    pub(crate) const fn from_u16(port: u16) -> Self {
        debug_assert!(port != 0, "port must not be zero");
        Self(port)
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl From<Port> for u16 {
    fn from(p: Port) -> Self {
        p.0
    }
}

impl PartialEq<u16> for Port {
    fn eq(&self, other: &u16) -> bool {
        self.0 == *other
    }
}

pub mod action;
pub mod app;
pub mod collection;
pub mod container;
pub mod deployment;
pub mod enums;
pub mod export;
pub mod file;
pub mod gate;
pub mod ingress;
pub mod job;
pub mod json;
pub mod param;
pub mod param_def;
pub mod pod;
pub mod resource;
pub mod service;
pub mod stdlib;
pub mod summary;
pub mod take;
pub mod version;
pub mod volume;

// spec: BSL#evaluation
pub fn register(engine: &mut Engine) {
    engine.build_type::<app::App>();
    engine.build_type::<param::Param>();
    engine.build_type::<file::File>();
    engine.build_type::<file::Directory>();
    engine.build_type::<service::Service>();
    engine.build_type::<service::HttpService>();
    engine.build_type::<service::HttpServiceRoute>();
    engine.build_type::<service::ServicePort>();
    engine.build_type::<service::ExternalService>();
    engine.build_type::<ingress::Ingress>();
    engine.build_type::<action::Action>();
    engine.build_type::<deployment::Deployment>();
    engine.build_type::<job::Job>();
    engine.build_type::<gate::Gate>();
    engine.build_type::<volume::Volume>();
    engine.build_type::<volume::ExternalVolume>();
    engine.build_type::<runtime::RuntimeInstance>();
    engine.build_type::<runtime::Started>();
    engine.build_type::<runtime::Termination>();
    engine.build_type::<runtime::Executed>();
    engine.build_type::<collection::Collection>();
    engine.build_type::<shell::ShellControl>();
    engine.build_type::<shell::ShellExit>();
    engine.build_type::<version::Version>();
    engine.build_type::<version::SemVer>();

    // spec: COL#coercion
    engine.register_fn("col", collection::col);

    // spec: STD#the-std-module
    engine.register_static_module("std", stdlib::module().into());

    // spec: BSL#evaluation
    // Refused at compile time: the optimiser drops an `import` whose alias is
    // unused, so a refusal when it runs would miss it.
    engine.disable_symbol("import");

    // spec: LIM#limits
    // Rhai's core package offers `sleep`, which no feature removes; the
    // engine's own functions are found before any package's.
    engine.register_fn(
        "sleep",
        |_: rhai::INT| -> Result<(), Box<rhai::EvalAltResult>> { Err(NO_SLEEP.into()) },
    );
    engine.register_fn(
        "sleep",
        |_: rhai::FLOAT| -> Result<(), Box<rhai::EvalAltResult>> { Err(NO_SLEEP.into()) },
    );
}

const NO_SLEEP: &str = "`sleep` is not available: evaluation cannot pause";

// spec: BSL#evaluation
// spec: BSL#enums
/// Check a definition once its script has evaluated in full: resolve what the
/// script left to the end of its evaluation, then check the rules it is held
/// to, which no single call could check because a later call can change what
/// an earlier one depends on.
// spec: BSL#evaluation
pub fn finish_evaluation(app: &app::App) -> Result<(), String> {
    gate::resolve_exclusive_selections(app)?;
    let def = app.def.load();
    ingress::check_redirect_terminations(&def)?;
    ingress::check_claim_conflicts(&def)?;
    container::check_images_declared(&def)?;
    service::check_route_balances(&def)?;
    param::check_defaults(&def)?;
    gate::check_gates(&def)
}

/// The scope a script is evaluated in, holding the constants for the host
/// facts as they stand, and the app it declares into.
///
/// Fails where a host constant the evaluation cannot stand without is
/// undetermined.
pub fn scope(
    bundle: Arc<crate::runtime::definition::Bundle>,
) -> Result<(Scope<'static>, app::App), String> {
    scope_with(&crate::sysconst::get(), bundle)
}

pub(crate) fn scope_with(
    facts: &crate::sysconst::SystemFacts,
    bundle: Arc<crate::runtime::definition::Bundle>,
) -> Result<(Scope<'static>, app::App), String> {
    // spec: CONST#host-constants
    // Undetermined memory or host name fails the evaluation rather than
    // standing in a value the script would plan around.
    let available_memory = facts
        .available_memory
        .clone()
        .map_err(|why| format!("AVAILABLE_MEMORY cannot be determined: {why}"))?;
    let node_name = facts
        .node_name
        .clone()
        .map_err(|why| format!("NODE_NAME cannot be determined: {why}"))?;

    let mut scope = Scope::new();

    // spec: CONST#host-constants
    scope.push_constant("AVAILABLE_THREADS", facts.available_threads.unwrap_or(1));

    // spec: CONST#host-constants
    scope.push_constant("AVAILABLE_MEMORY", available_memory);

    // spec: CONST#host-constants
    scope.push_constant("CPU_ARCHITECTURE", std::env::consts::ARCH.to_owned());

    // spec: CONST#host-constants
    scope.push_constant("HOST_HAS_IPV4", facts.ipv4_egress);

    // spec: CONST#host-constants
    scope.push_constant("HOST_HAS_IPV6", facts.ipv6_egress);

    // spec: CONST#host-constants
    scope.push_constant("NAT64_ACTIVE", facts.nat64_active);

    // spec: CONST#host-constants
    scope.push_constant("HAS_SNAPSHOTS", facts.has_snapshots);

    // spec: CONST#host-constants
    scope.push_constant("NODE_NAME", node_name);

    // spec: CONST#host-constants
    scope.push_constant("TIMEZONE", facts.timezone.clone());

    // spec: CONST#idle-command
    // spec: LINUX#idle-command
    let idle_cmd: rhai::Array = vec![
        rhai::Dynamic::from("sleep".to_owned()),
        rhai::Dynamic::from("infinity".to_owned()),
    ];
    scope.push_constant("IDLE_CMD", idle_cmd);

    // spec: CONST#onupdate
    scope.push_constant("OnUpdate", enums::OnUpdate::rhai_constant());

    // spec: CONST#onterminate
    scope.push_constant("OnTerminate", enums::OnTerminate::rhai_constant());

    // spec: CONST#onexit
    scope.push_constant("OnExit", enums::OnExit::rhai_constant());

    // spec: CONST#priority
    scope.push_constant("Priority", enums::Priority::rhai_constant());

    // spec: CONST#resourcetype
    scope.push_constant("ResourceType", resource::ResourceKind::rhai_constant());

    // spec: ING#terminate
    scope.push_constant("Terminate", enums::Terminate::rhai_constant());

    // spec: ING#output
    scope.push_constant("Output", enums::Output::rhai_constant());

    let app = app::App::with_bundle(bundle);
    // spec: APP#the-app-variable
    scope.push("app", app.clone());
    Ok((scope, app))
}
