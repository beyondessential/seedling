use std::collections::{BTreeMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};

use rhai::{CustomType, EvalAltResult, Map, NativeCallContext, TypeBuilder};
use seedling_protocol::names::{ActionName, AppName, ParamName, ShellName};

use super::install::ParamDef;

// l[impl action.option-params]
#[derive(Debug, Clone)]
pub struct ActionDef {
    pub name: ActionName,
    pub description: Option<String>,
    // l[impl action.schedule]
    pub schedules: Vec<String>,
    pub params: BTreeMap<ParamName, ParamDef>,
}

/// Compute a stable hash from `(app_name, action_name)` for use with cronexpr's
/// `H` extension.
pub fn schedule_hash(app_name: &AppName, action_name: &ActionName) -> u64 {
    let mut hasher = DefaultHasher::new();
    app_name.as_str().hash(&mut hasher);
    action_name.as_str().hash(&mut hasher);
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

// l[impl action.type]
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
            // l[impl action.schedule]
            // r[impl schedule.start-reject]
            .with_fn(
                "on_schedule",
                |this: &mut Self, expr: &str| -> Result<Self, Box<EvalAltResult>> {
                    if this.name == "start" {
                        return Err("on_schedule cannot be called on the start action".into());
                    }
                    validate_cron_expr(expr, &this.app_name, &this.name)
                        .map_err(|e| -> Box<EvalAltResult> { e.into() })?;
                    super::app::append_action_schedule(&this.name, expr);
                    Ok(this.clone())
                },
            )
            // l[impl action.call]
            .with_fn(
                "call",
                |ctx: NativeCallContext, this: &mut Self| -> Result<(), Box<EvalAltResult>> {
                    crate::runtime::barrier::action_call::call_action(&ctx, &this.name, Map::new())
                },
            )
            .with_fn(
                "call",
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
