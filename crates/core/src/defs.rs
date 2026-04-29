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
/// modification of static resource references inside action closures.
pub trait Freezable {
    fn is_frozen(&self) -> bool;

    fn ensure_unfrozen(&self) -> Result<(), Box<rhai::EvalAltResult>> {
        if self.is_frozen() {
            Err("cannot modify a static resource reference inside an action closure".into())
        } else {
            Ok(())
        }
    }
}

// l[impl bsl.name]
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

// l[impl bsl.port]
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
pub mod ingress;
pub mod install;
pub mod job;
pub mod param;
pub mod pod;
pub mod resource;
pub mod service;
pub mod summary;
pub mod volume;

// l[impl bsl.syntax]
// l[impl bsl.script]
pub fn register(engine: &mut Engine) {
    engine.build_type::<app::App>();
    engine.build_type::<param::Param>();
    engine.build_type::<service::Service>();
    engine.build_type::<service::HttpService>();
    engine.build_type::<service::HttpServiceRoute>();
    engine.build_type::<service::ServicePort>();
    engine.build_type::<service::ExternalService>();
    engine.build_type::<ingress::Ingress>();
    engine.build_type::<action::Action>();
    engine.build_type::<deployment::Deployment>();
    engine.build_type::<job::Job>();
    engine.build_type::<volume::Volume>();
    engine.build_type::<volume::ExternalVolume>();
    engine.build_type::<runtime::RuntimeInstance>();
    engine.build_type::<runtime::Started>();
    engine.build_type::<runtime::Termination>();
    engine.build_type::<runtime::Executed>();
    engine.build_type::<collection::Collection>();
    engine.build_type::<shell::ShellControl>();

    // l[impl collection.col]
    engine.register_fn("col", collection::col);
}

/// Best-effort count of compute threads available to the daemon process.
///
/// `std::thread::available_parallelism` honours cgroup CPU quotas, sched
/// affinity, and platform limits, which matches the spec's framing
/// ("amount of compute threads available to the application", not
/// "total cores on the host"). Falls back to `1` on the rare platform
/// where the query is unsupported, which keeps the constant within its
/// spec invariant of positive non-zero.
fn available_threads() -> i64 {
    std::thread::available_parallelism()
        .map(|n| i64::try_from(n.get()).unwrap_or(i64::MAX))
        .unwrap_or(1)
}

/// Memory the script may plan around: 90% of the host's `MemTotal`,
/// reserving 10% for the daemon itself, the runtime, and the kernel's
/// page cache. Always at least `1` byte to keep the
/// `AVAILABLE_MEMORY > 0` invariant from the spec.
fn available_memory_bytes() -> i64 {
    (read_mem_total_bytes().saturating_mul(9) / 10).max(1)
}

/// Read `MemTotal` from `/proc/meminfo` and return the value in bytes.
///
/// Falls back to `1` (the minimum positive value permitted by the spec)
/// if the file cannot be read or parsed.
fn read_mem_total_bytes() -> i64 {
    let Ok(contents) = std::fs::read_to_string("/proc/meminfo") else {
        return 1;
    };
    for line in contents.lines() {
        let Some(rest) = line.strip_prefix("MemTotal:") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let Some(num) = parts.next() else { continue };
        let Some(unit) = parts.next() else { continue };
        let Ok(value) = num.parse::<i64>() else {
            continue;
        };
        let multiplier: i64 = match unit {
            "B" => 1,
            "kB" | "KB" => 1024,
            "MB" => 1024 * 1024,
            "GB" => 1024 * 1024 * 1024,
            _ => continue,
        };
        return value.saturating_mul(multiplier).max(1);
    }
    1
}

// l[impl bsl.scope]
// l[impl bsl.enums]
pub fn scope() -> (Scope<'static>, app::App) {
    let mut scope = Scope::new();
    let facts = crate::sysconst::get();

    // l[impl const.available-threads]
    scope.push_constant("AVAILABLE_THREADS", available_threads());

    // l[impl const.available-memory]
    scope.push_constant("AVAILABLE_MEMORY", available_memory_bytes());

    // l[impl const.cpu-architecture]
    scope.push_constant("CPU_ARCHITECTURE", std::env::consts::ARCH.to_owned());

    // l[impl const.host-has-ipv4]
    scope.push_constant("HOST_HAS_IPV4", facts.ipv4_egress);

    // l[impl const.host-has-ipv6]
    scope.push_constant("HOST_HAS_IPV6", facts.ipv6_egress);

    // l[impl const.nat64-active]
    scope.push_constant("NAT64_ACTIVE", facts.nat64_active);

    // l[impl const.has-snapshots]
    scope.push_constant("HAS_SNAPSHOTS", facts.has_snapshots);

    // l[impl const.node-name]
    scope.push_constant("NODE_NAME", facts.node_name.clone());

    // l[impl const.timezone]
    scope.push_constant("TIMEZONE", facts.timezone.clone());

    // l[impl const.idle-cmd]
    let idle_cmd: rhai::Array = vec![
        rhai::Dynamic::from("sleep".to_owned()),
        rhai::Dynamic::from("infinity".to_owned()),
    ];
    scope.push_constant("IDLE_CMD", idle_cmd);

    // l[impl const.on-update.rolling]
    // l[impl const.on-update.replace]
    scope.push_constant("OnUpdate", enums::OnUpdate::rhai_constant());

    // l[impl const.on-terminate.recreate]
    scope.push_constant("OnTerminate", enums::OnTerminate::rhai_constant());

    // l[impl const.on-exit.restart]
    // l[impl const.on-exit.terminate]
    // l[impl const.on-exit.restart-on-failure]
    scope.push_constant("OnExit", enums::OnExit::rhai_constant());

    // l[impl const.resource-type.enum]
    scope.push_constant("ResourceType", resource::ResourceKind::rhai_constant());

    // l[impl const.terminate.tls]
    // l[impl const.terminate.dtls]
    // l[impl const.terminate.https]
    scope.push_constant("Terminate", enums::Terminate::rhai_constant());

    // l[impl const.output.tcp]
    // l[impl const.output.udp]
    // l[impl const.output.http1]
    // l[impl const.output.http2]
    scope.push_constant("Output", enums::Output::rhai_constant());

    let app = app::App::default();
    // l[impl app.var]
    scope.push("app", app.clone());
    (scope, app)
}
