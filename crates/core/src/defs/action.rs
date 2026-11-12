use std::collections::BTreeMap;
use std::hash::Hasher;

use rhai::{CustomType, EvalAltResult, Map, NativeCallContext, TypeBuilder};
use seedling_protocol::names::{ActionName, AppName, ParamName, ShellName};
use siphasher::sip::SipHasher13;

use super::install::ParamDef;

// spec: ACN#param-schemas
#[derive(Debug, Clone)]
pub struct ActionDef {
    pub name: ActionName,
    pub description: Option<String>,
    // spec: ACN#scheduled-actions
    pub schedules: Vec<String>,
    pub params: BTreeMap<ParamName, ParamDef>,
}

/// Compute a stable hash from `(app_name, action_name)` for use with cronexpr's
/// `H` extension.
///
/// Unseeded SipHash-1-3 over each name's bytes followed by a `0xff` terminator.
/// Both the algorithm and the byte stream are fixed here rather than left to
/// `std::hash`, whose `DefaultHasher` makes no promise across releases: a
/// toolchain change must never move an `H` schedule.
// spec: ACN#scheduled-actions
pub fn schedule_hash(app_name: &AppName, action_name: &ActionName) -> u64 {
    let mut hasher = SipHasher13::new();
    for name in [app_name.as_str(), action_name.as_str()] {
        hasher.write(name.as_bytes());
        hasher.write_u8(0xff);
    }
    hasher.finish()
}

/// Validate and parse a 5-field cron expression. The `H` extension is supported
/// using the hash derived from the given app and action names. The timezone
/// defaults to the system local timezone when omitted.
pub fn validate_cron_expr(
    expr: &str,
    app_name: &AppName,
    action_name: &ActionName,
) -> Result<(), String> {
    let opts = cron_parse_options(app_name, action_name);
    cronexpr::parse_crontab_with(expr, opts)
        .map(|_| ())
        .map_err(|e| format!("invalid cron expression '{expr}': {e}"))
}

/// Parse a cron expression for schedule evaluation. Returns the `Crontab` ready
/// for `find_next` calls.
pub fn parse_cron_expr(
    expr: &str,
    app_name: &AppName,
    action_name: &ActionName,
) -> Result<cronexpr::Crontab, cronexpr::Error> {
    let opts = cron_parse_options(app_name, action_name);
    cronexpr::parse_crontab_with(expr, opts)
}

fn cron_parse_options(app_name: &AppName, action_name: &ActionName) -> cronexpr::ParseOptions {
    let mut opts = cronexpr::ParseOptions::default();
    opts.fallback_timezone_option = cronexpr::FallbackTimezoneOption::System;
    opts.hashed_value = Some(schedule_hash(app_name, action_name));
    opts
}

// spec: ACN#action-declaration
#[derive(Debug, Clone)]
pub struct Action {
    pub name: ActionName,
    app_name: AppName,
}

impl Action {
    pub fn new(name: ActionName, app_name: AppName) -> Self {
        Self { name, app_name }
    }
}

impl CustomType for Action {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Action")
            // spec: ACN#scheduled-actions
            // spec: ACN#start-action
            .with_fn(
                "on_schedule",
                |this: &mut Self, expr: &str| -> Result<Self, Box<EvalAltResult>> {
                    if crate::runtime::barrier::runtime::is_in_action_closure() {
                        return Err("on_schedule cannot be called inside an action closure".into());
                    }
                    if this.name == "start" {
                        return Err("on_schedule cannot be called on the start action".into());
                    }
                    validate_cron_expr(expr, &this.app_name, &this.name)
                        .map_err(|e| -> Box<EvalAltResult> { e.into() })?;
                    super::app::append_action_schedule(&this.name, expr);
                    Ok(this.clone())
                },
            )
            // spec: ACN#action-invocation
            // The Rhai engine special-cases `.call(...)` for function-pointer
            // dispatch and intercepts it before user-registered methods are
            // looked up (see make_method_call in rhai/src/func/call.rs), so
            // this surface uses `.invoke(...)` instead.
            .with_fn(
                "invoke",
                |ctx: NativeCallContext, this: &mut Self| -> Result<(), Box<EvalAltResult>> {
                    crate::runtime::barrier::action_call::call_action(&ctx, &this.name, Map::new())
                },
            )
            .with_fn(
                "invoke",
                |ctx: NativeCallContext,
                 this: &mut Self,
                 params: Map|
                 -> Result<(), Box<EvalAltResult>> {
                    crate::runtime::barrier::action_call::call_action(&ctx, &this.name, params)
                },
            );
    }
}

#[derive(Debug, Clone)]
pub struct ShellDef {
    pub name: ShellName,
    pub description: Option<String>,
    pub params: BTreeMap<ParamName, ParamDef>,
}
