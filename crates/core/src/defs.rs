// BSL definition structs carry fields that the runtime will consume once the
// reconciliation loop is wired up. Until then, suppress dead_code warnings.
#![allow(dead_code)]

use std::{collections::BTreeMap, sync::Arc};

use parking_lot::Mutex;
use rhai::{AST, Dynamic, Engine, EvalAltResult, OptimizationLevel, Scope};

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
pub mod compare;
pub mod container;
pub mod deployment;
pub mod duration;
pub mod enums;
pub mod export;
pub mod file;
pub mod gate;
pub mod ingress;
pub mod ip;
pub mod job;
pub mod json;
pub mod param;
pub mod param_def;
pub mod pod;
pub mod purl;
pub mod quantity;
pub mod regex;
pub mod resource;
pub mod service;
pub mod stdlib;
pub mod summary;
pub mod take;
pub mod url;
pub mod version;
pub mod volume;

// spec: BSL#evaluation
pub fn register(engine: &mut Engine) {
    engine.build_type::<app::App>();
    engine.build_type::<regex::Regex>();
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
    engine.build_type::<ip::IpAddress>();
    engine.build_type::<ip::IpSubnet>();
    engine.build_type::<ip::IpRange>();
    engine.build_type::<duration::Duration>();

    // spec: COL#coercion
    engine.register_fn("col", collection::col);

    // spec: STD#the-std-module
    engine.register_static_module(stdlib::MODULE, stdlib::module().into());

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

    // spec: FMT#json
    // Rhai's basic map package offers `to_json`, which no feature removes.
    engine.register_fn(
        "to_json",
        |_: &mut rhai::Map| -> Result<String, Box<rhai::EvalAltResult>> { Err(NO_TO_JSON.into()) },
    );
}

const NO_SLEEP: &str = "`sleep` is not available: evaluation cannot pause";
const NO_TO_JSON: &str = "`to_json` is not available: use `std::encode_json`";

/// Compile `source` for an engine [`crate::setup_language`] built, then
/// optimise it as Rhai would.
// spec: REX#compiling-a-pattern
/// Refuses a script that calls `std::regex` or `std::parse_regex` with a
/// string literal that helper would throw for, whether or not the call
/// would run. The check reads the script before optimisation, which would
/// carry a `const` into a call's argument as though it were written there.
pub fn compile(engine: &Engine, source: &str) -> Result<AST, Box<EvalAltResult>> {
    debug_assert_eq!(
        engine.optimization_level(),
        OptimizationLevel::None,
        "an engine setup_language did not build would skip the literal-pattern check's premise"
    );
    #[expect(
        clippy::disallowed_methods,
        reason = "the one place a script is compiled"
    )]
    let ast = engine.compile(source)?;
    regex::check_literal_calls(&ast, &regex::engine_limits(engine)?)?;
    Ok(engine.optimize_ast(&Scope::new(), ast, OptimizationLevel::Simple))
}

// spec: BSL#evaluation
// spec: BSL#enums
/// Check a definition once its script has evaluated in full: resolve what the
/// script left to the end of its evaluation, then check the rules it is held
/// to, which no single call could check because a later call can change what
/// an earlier one depends on.
// spec: BSL#evaluation
pub fn finish_evaluation(app: &app::App, limits: &crate::ScriptLimits) -> Result<(), String> {
    gate::resolve_exclusive_selections(app)?;
    let def = app.def.load();
    ingress::check_redirect_terminations(&def)?;
    ingress::check_claim_conflicts(&def)?;
    container::check_images_declared(&def)?;
    service::check_route_balances(&def)?;
    param::check_params(&def, &limits.helpers())?;
    gate::check_gates(&def)
}

/// The constants and enums every evaluation reads, by name, with the host
/// constants as `facts` holds them.
///
/// Fails where a host constant the evaluation cannot stand without is
/// undetermined, unless `facts` has such a constant fail only where it is
/// read, in which case it is left out.
pub fn constants(
    facts: &crate::sysconst::SystemFacts,
) -> Result<BTreeMap<&'static str, Dynamic>, String> {
    // spec: CONST#host-constants
    // Undetermined memory or host name fails the evaluation rather than
    // standing in a value the script would plan around.
    if facts.undetermined == crate::sysconst::Undetermined::FailsEvaluation
        && let Some(why) = crate::sysconst::UNDETERMINABLE
            .iter()
            .find_map(|name| facts.why_undetermined(name))
    {
        return Err(why);
    }

    let mut constants = BTreeMap::new();

    // spec: CONST#host-constants
    constants.insert(
        "AVAILABLE_THREADS",
        Dynamic::from(facts.available_threads.unwrap_or(1)),
    );

    // spec: CONST#host-constants
    if let Ok(memory) = facts.available_memory {
        constants.insert("AVAILABLE_MEMORY", Dynamic::from(memory));
    }

    // spec: CONST#host-constants
    constants.insert(
        "CPU_ARCHITECTURE",
        Dynamic::from(facts.cpu_architecture.clone()),
    );

    // spec: CONST#host-constants
    constants.insert("HOST_HAS_IPV4", Dynamic::from(facts.ipv4_egress));

    // spec: CONST#host-constants
    constants.insert("HOST_HAS_IPV6", Dynamic::from(facts.ipv6_egress));

    // spec: CONST#host-constants
    constants.insert("NAT64_ACTIVE", Dynamic::from(facts.nat64_active));

    // spec: CONST#host-constants
    constants.insert("HAS_SNAPSHOTS", Dynamic::from(facts.has_snapshots));

    // spec: CONST#host-constants
    if let Ok(name) = &facts.node_name {
        constants.insert("NODE_NAME", Dynamic::from(name.clone()));
    }

    // spec: CONST#host-constants
    constants.insert("TIMEZONE", Dynamic::from(facts.timezone.clone()));

    // spec: CONST#idle-command
    // spec: LINUX#idle-command
    let idle_cmd: rhai::Array = ["sh", "-c", "trap 'exit 0' TERM INT; sleep infinity & wait"]
        .into_iter()
        .map(|arg| rhai::Dynamic::from(arg.to_owned()))
        .collect();
    constants.insert("IDLE_CMD", Dynamic::from(idle_cmd));

    // spec: CONST#onupdate
    constants.insert("OnUpdate", Dynamic::from(enums::OnUpdate::rhai_constant()));

    // spec: CONST#onterminate
    constants.insert(
        "OnTerminate",
        Dynamic::from(enums::OnTerminate::rhai_constant()),
    );

    // spec: CONST#onexit
    constants.insert("OnExit", Dynamic::from(enums::OnExit::rhai_constant()));

    // spec: CONST#priority
    constants.insert("Priority", Dynamic::from(enums::Priority::rhai_constant()));

    // spec: CONST#resourcetype
    constants.insert(
        "ResourceType",
        Dynamic::from(resource::ResourceKind::rhai_constant()),
    );

    // spec: ING#terminate
    constants.insert(
        "Terminate",
        Dynamic::from(enums::Terminate::rhai_constant()),
    );

    // spec: ING#output
    constants.insert("Output", Dynamic::from(enums::Output::rhai_constant()));

    Ok(constants)
}

/// Have `engine` supply the variables a script reads without declaring them:
/// the constants and enums, from `facts` as they stand now, and the `app`
/// handle from the scope.
///
/// Fails as [`constants`] does.
pub fn resolve_variables(
    engine: &mut Engine,
    facts: &crate::sysconst::SystemFacts,
) -> Result<(), String> {
    let constants = constants(facts)?;
    let undetermined: Vec<(&str, String)> = crate::sysconst::UNDETERMINABLE
        .iter()
        .filter_map(|name| Some((*name, facts.why_undetermined(name)?)))
        .collect();
    #[expect(
        deprecated,
        reason = "Rhai marks on_var deprecated only to flag it as volatile"
    )]
    engine.on_var(move |name, _, ctx| {
        // spec: APP#the-app-variable
        // A closure captures a scope variable as one shared cell, and a method
        // chain holds a lock on a shared root for its whole length, so a
        // closure invoked from `app.action(..).invoke(..)` would find `app`
        // locked. Rhai never shares a resolved variable, so each closure
        // captures its own copy of the handle; every copy holds the same app.
        if name == "app" {
            return Ok(ctx
                .scope()
                .get(name)
                .filter(|value| value.is::<app::App>())
                .cloned());
        }
        // spec: CONST#constant-lifetime
        // A function body never sees the script's scope, so the constants are
        // supplied here rather than placed in it. Rhai holds a resolved
        // variable read-only, as it does a scope constant, and a variable or
        // parameter of the same name shadows it.
        if let Some(value) = constants.get(name) {
            return Ok((!ctx.scope().contains(name)).then(|| value.clone()));
        }
        // spec: CLI#check-host-constants
        // A constant left out as undetermined fails where it is read.
        if let Some((_, why)) = undetermined.iter().find(|(n, _)| *n == name)
            && !ctx.scope().contains(name)
        {
            return Err(why.clone().into());
        }
        Ok(None)
    });
    Ok(())
}

/// The scope a script is evaluated in, holding the app it declares into.
pub fn scope(bundle: Arc<seedling_bundle::Bundle>) -> (Scope<'static>, app::App) {
    let mut scope = Scope::new();
    let app = app::App::with_bundle(bundle);
    // spec: APP#the-app-variable
    scope.push("app", app.clone());
    (scope, app)
}
