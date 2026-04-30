//! Inject Seedling-side action breadcrumbs into the systemd journal so they
//! appear in `apps logs` alongside container output.
//!
//! Each breadcrumb is a single journald record carrying the same
//! `SEEDLING_APP` / `SEEDLING_RESOURCE` / `SEEDLING_INSTANCE` fields the
//! containers' stdout/stderr already get tagged with, plus a
//! `SEEDLING_RT_CALL` field naming the rt.* primitive (or a synthetic
//! kind for unit creation and replay markers). The journal reader in
//! `system::journal` picks them up via the same field-match logic, so an
//! operator running `seedling-ctl apps logs <app>` sees the closure's
//! call sequence interleaved with the container output it produced.
//!
//! Breadcrumbs are emitted on the *first fresh execution* of a call, not
//! on subsequent replays, so a barrier-suspended operation doesn't flood
//! the journal. Replay-pass boundaries surface as a single
//! [`BreadcrumbKind::Replay`] line at the top of each replay attempt.

use std::path::Path;

use seedling_protocol::names::{ActionName, AppName};
use serde_json::Map as JsonMap;
use serde_json::Value as JsonValue;

use crate::runtime::ResourceInstance;
use crate::runtime::barrier::VolumeWriteTarget;

/// Names the rt.* call (or synthetic event) for the journal record.
///
/// Each variant maps to a `SEEDLING_RT_CALL` value and a human-readable
/// `MESSAGE`. The kind also drives which `SEEDLING_RESOURCE` /
/// `SEEDLING_INSTANCE` tags (if any) get attached, so that filtering
/// `apps logs --resource X` shows the breadcrumbs that touch X.
pub enum BreadcrumbKind<'a> {
    Start {
        resources: &'a [ResourceInstance],
    },
    Stop {
        resources: &'a [ResourceInstance],
    },
    Query {
        resources: &'a [ResourceInstance],
    },
    WarmCerts {
        resources: &'a [ResourceInstance],
    },
    WarmImages {
        refs: &'a [String],
    },
    Restart {
        target: &'a ResourceInstance,
    },
    Exec {
        target: &'a ResourceInstance,
        argv: &'a [String],
    },
    Signal {
        target: &'a ResourceInstance,
        signal: &'a str,
    },
    Write {
        target: &'a VolumeWriteTarget,
        path: &'a str,
        len: usize,
    },
    SubAction {
        name: &'a ActionName,
        params: &'a JsonMap<String, JsonValue>,
    },
    /// One per systemd unit Seedling creates. `source_call` describes
    /// the rt.* call that produced the unit (e.g. `Start(api)` or
    /// `Start(<DB provisioning>)` for an anonymous resource).
    UnitCreate {
        unit: &'a str,
        source_call: &'a str,
    },
    /// One at the top of each replay pass through `run_operation`.
    Replay {
        operation_id: &'a str,
        committed_len: usize,
    },
}

/// Fields carried with every breadcrumb.
pub struct Breadcrumb<'a> {
    pub app: Option<&'a AppName>,
    pub kind: BreadcrumbKind<'a>,
    /// Source position from Rhai when the rt.* call site is known.
    /// Format: `<file>:<line>:<col>`.
    pub script_pos: Option<rhai::Position>,
}

impl Breadcrumb<'_> {
    /// Send the breadcrumb to journald. Silently no-ops if journald is
    /// unavailable (dev runs outside systemd).
    pub fn emit(&self) {
        // Build the per-target record set. Each target produces one
        // journal entry with its SEEDLING_RESOURCE / SEEDLING_INSTANCE
        // tags so per-resource log queries pick the breadcrumb up.
        let targets: Vec<Target<'_>> = match &self.kind {
            BreadcrumbKind::Start { resources }
            | BreadcrumbKind::Stop { resources }
            | BreadcrumbKind::Query { resources }
            | BreadcrumbKind::WarmCerts { resources } => {
                resources.iter().map(Target::for_instance).collect()
            }
            BreadcrumbKind::Restart { target }
            | BreadcrumbKind::Exec { target, .. }
            | BreadcrumbKind::Signal { target, .. } => vec![Target::for_instance(target)],
            BreadcrumbKind::Write { target, .. } => match target {
                VolumeWriteTarget::NamedVolume { name, .. } => vec![Target {
                    resource: Some(name.as_str()),
                    resource_kind: Some("volume"),
                    instance: None,
                }],
                VolumeWriteTarget::AnonymousVolume { .. }
                | VolumeWriteTarget::ExternalBound { .. } => vec![Target::empty()],
            },
            BreadcrumbKind::WarmImages { .. }
            | BreadcrumbKind::SubAction { .. }
            | BreadcrumbKind::Replay { .. } => vec![Target::empty()],
            BreadcrumbKind::UnitCreate { unit, .. } => vec![Target {
                resource: None,
                resource_kind: None,
                instance: Some(unit),
            }],
        };

        let call = self.kind.rt_call();
        let message = self.kind.message();
        let pos = self.script_pos.and_then(|p| {
            if p.is_none() {
                None
            } else {
                Some(format!(
                    "{}:{}",
                    p.line().unwrap_or(0),
                    p.position().unwrap_or(0)
                ))
            }
        });

        for t in targets {
            send_record(self.app, &t, call, &message, pos.as_deref());
        }
    }
}

struct Target<'a> {
    resource: Option<&'a str>,
    resource_kind: Option<&'a str>,
    instance: Option<&'a str>,
}

impl<'a> Target<'a> {
    fn empty() -> Self {
        Self {
            resource: None,
            resource_kind: None,
            instance: None,
        }
    }

    fn for_instance(inst: &'a ResourceInstance) -> Self {
        let resource_kind = match inst.kind {
            crate::defs::resource::ResourceKind::Service => "service",
            crate::defs::resource::ResourceKind::HttpService => "http_service",
            crate::defs::resource::ResourceKind::Ingress => "ingress",
            crate::defs::resource::ResourceKind::Deployment => "deployment",
            crate::defs::resource::ResourceKind::Job => "job",
            crate::defs::resource::ResourceKind::Volume => "volume",
            crate::defs::resource::ResourceKind::ExternalVolume => "external_volume",
            crate::defs::resource::ResourceKind::ExternalService => "external_service",
            crate::defs::resource::ResourceKind::Action => "action",
            crate::defs::resource::ResourceKind::Parameter => "parameter",
        };
        Self {
            // For named resources we use the BSL-level name; for
            // anonymous resources we fall back to the display name so
            // operators can still match on `--resource <display>` or
            // pivot from the unit-create breadcrumb.
            resource: Some(inst.name.as_deref().unwrap_or(&inst.display_name)),
            resource_kind: Some(resource_kind),
            instance: Some(&inst.display_name),
        }
    }
}

impl BreadcrumbKind<'_> {
    fn rt_call(&self) -> &'static str {
        match self {
            BreadcrumbKind::Start { .. } => "start",
            BreadcrumbKind::Stop { .. } => "stop",
            BreadcrumbKind::Query { .. } => "query",
            BreadcrumbKind::WarmCerts { .. } => "warm_certs",
            BreadcrumbKind::WarmImages { .. } => "warm_images",
            BreadcrumbKind::Restart { .. } => "restart",
            BreadcrumbKind::Exec { .. } => "exec",
            BreadcrumbKind::Signal { .. } => "signal",
            BreadcrumbKind::Write { .. } => "write",
            BreadcrumbKind::SubAction { .. } => "sub_action",
            BreadcrumbKind::UnitCreate { .. } => "unit_create",
            BreadcrumbKind::Replay { .. } => "replay",
        }
    }

    fn message(&self) -> String {
        match self {
            BreadcrumbKind::Start { resources } => {
                format!("rt.start{}", fmt_resources(resources))
            }
            BreadcrumbKind::Stop { resources } => {
                format!("rt.stop{}", fmt_resources(resources))
            }
            BreadcrumbKind::Query { resources } => {
                format!("rt.query{}", fmt_resources(resources))
            }
            BreadcrumbKind::WarmCerts { resources } => {
                format!("rt.warm_certs{}", fmt_resources(resources))
            }
            BreadcrumbKind::WarmImages { refs } => {
                format!("rt.warm_images([{}])", refs.join(", "))
            }
            BreadcrumbKind::Restart { target } => {
                format!("rt.restart({})", target_label(target))
            }
            BreadcrumbKind::Exec { target, argv } => format!(
                "rt.exec({}, [{}])",
                target_label(target),
                argv.iter()
                    .map(|s| quote_short(s))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            BreadcrumbKind::Signal { target, signal } => {
                format!("rt.signal({}, {})", target_label(target), signal)
            }
            BreadcrumbKind::Write { target, path, len } => {
                format!(
                    "rt.write({}, {}, len={})",
                    fmt_write_target(target),
                    path,
                    len
                )
            }
            BreadcrumbKind::SubAction { name, params } => {
                let summary = if params.is_empty() {
                    String::new()
                } else {
                    format!(
                        " {}",
                        serde_json::to_string(params).unwrap_or_else(|_| "{}".into())
                    )
                };
                format!("Action.invoke({}){summary}", name.as_str())
            }
            BreadcrumbKind::UnitCreate { unit, source_call } => {
                format!("created unit {unit} from {source_call}")
            }
            BreadcrumbKind::Replay {
                operation_id,
                committed_len,
            } => {
                format!("replay pass: op={operation_id} committed={committed_len}")
            }
        }
    }
}

fn fmt_resources(resources: &[ResourceInstance]) -> String {
    if resources.is_empty() {
        return "()".into();
    }
    let names: Vec<String> = resources.iter().map(target_label).collect();
    format!("({})", names.join(", "))
}

fn target_label(inst: &ResourceInstance) -> String {
    inst.name
        .clone()
        .unwrap_or_else(|| inst.display_name.clone())
}

fn fmt_write_target(target: &VolumeWriteTarget) -> String {
    match target {
        VolumeWriteTarget::NamedVolume { name, .. } => name.clone(),
        VolumeWriteTarget::AnonymousVolume { anon_id, .. } => format!("<anon:{anon_id}>"),
        VolumeWriteTarget::ExternalBound { host_path } => {
            // Trim the path prefix for readability; operators see the
            // tail in journals already (the volume mount path).
            Path::new(host_path)
                .file_name()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
                .unwrap_or_else(|| host_path.display().to_string())
        }
    }
}

fn quote_short(s: &str) -> String {
    // Single-quote only when whitespace or commas would confuse the
    // bracket render. Keep the breadcrumb readable in plain output.
    if s.chars().any(|c| c.is_whitespace() || c == ',') {
        format!("'{s}'")
    } else {
        s.to_owned()
    }
}

fn send_record(
    app: Option<&AppName>,
    target: &Target<'_>,
    rt_call: &str,
    message: &str,
    pos: Option<&str>,
) {
    // PRIORITY=6 (informational). We log Seedling-side flow at the
    // same severity systemd uses for unit transitions; faults stay at
    // their own (higher) priority via the existing fault path.
    let mut fields: Vec<String> = vec![
        "PRIORITY=6".to_owned(),
        format!("MESSAGE={message}"),
        format!("SEEDLING_RT_CALL={rt_call}"),
    ];
    if let Some(app) = app {
        fields.push(format!("SEEDLING_APP={app}"));
    }
    if let Some(r) = target.resource {
        fields.push(format!("SEEDLING_RESOURCE={r}"));
    }
    if let Some(rk) = target.resource_kind {
        fields.push(format!("SEEDLING_RESOURCE_KIND={rk}"));
    }
    if let Some(i) = target.instance {
        fields.push(format!("SEEDLING_INSTANCE={i}"));
    }
    if let Some(p) = pos {
        fields.push(format!("SEEDLING_SCRIPT_POS={p}"));
    }
    let refs: Vec<&str> = fields.iter().map(String::as_str).collect();
    // sd_journal_send returns negative on error (e.g. journald not
    // running). We don't surface those: dev environments without
    // systemd just lose the breadcrumbs, which is harmless.
    let _ = systemd::journal::send(&refs);
}
