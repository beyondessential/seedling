//! A parser for every parameter kind, `std::parse_<kind>`.

use rhai::{Dynamic, EvalAltResult, FuncRegistration, NativeCallContext};
use strum::VariantArray;

use crate::defs::{
    ip::take_family,
    param_def::{
        IntRange, ParamKind, ParamKindName, take_duration_arguments, take_options, take_range,
    },
    regex::with_evaluation_budget,
    take::take_string,
};

// spec: ACN#kind-parsers
pub(super) fn register(module: &mut rhai::Module) {
    for &name in ParamKindName::VARIANTS {
        let helper = helper_name(name);
        match name {
            // spec: ACN#kind-parsers
            ParamKindName::Integer => {
                register_ranged_parser(module, &helper, |range| ParamKind::Integer { range });
            }
            // spec: ACN#kind-parsers
            ParamKindName::Quantity => {
                register_ranged_parser(module, &helper, |range| ParamKind::Quantity { range });
            }
            // spec: DUR#bounds
            ParamKindName::Duration => {
                register_parser(module, &helper, ParamKind::Duration(Default::default()));
                FuncRegistration::new(&helper).set_into_module(
                    module,
                    |ctx: NativeCallContext,
                     value: Dynamic,
                     arguments: Dynamic|
                     -> Result<Dynamic, Box<EvalAltResult>> {
                        let bounds = take_duration_arguments("arguments", arguments)?;
                        parse(&ctx, &ParamKind::Duration(bounds), value)
                    },
                );
            }
            // spec: ACN#kind-parsers
            ParamKindName::Choice => {
                FuncRegistration::new(&helper).set_into_module(
                    module,
                    |_: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> {
                        Err("std::parse_choice takes the value and its `options`".into())
                    },
                );
                FuncRegistration::new(&helper).set_into_module(
                    module,
                    |ctx: NativeCallContext,
                     value: Dynamic,
                     options: Dynamic|
                     -> Result<Dynamic, Box<EvalAltResult>> {
                        let options = take_options("options", options)?;
                        parse(&ctx, &ParamKind::Choice { options }, value)
                    },
                );
            }
            // spec: IP#family-argument
            ParamKindName::IpAddress | ParamKindName::IpSubnet | ParamKindName::IpRange => {
                let kind = ParamKind::without_arguments(name).expect("family is optional");
                register_parser(module, &helper, kind);
                FuncRegistration::new(&helper).set_into_module(
                    module,
                    move |ctx: NativeCallContext,
                          value: Dynamic,
                          family: Dynamic|
                          -> Result<Dynamic, Box<EvalAltResult>> {
                        let family = take_family("family", family)?;
                        let kind = ParamKind::of_family(name, family).expect("an IP kind");
                        parse(&ctx, &kind, value)
                    },
                );
            }
            _ => {
                let kind =
                    ParamKind::without_arguments(name).expect("only choice requires an argument");
                register_parser(module, &helper, kind);
            }
        }
    }
}

fn register_parser(module: &mut rhai::Module, helper: &str, kind: ParamKind) {
    // `std::parse_regex` counts toward the evaluation's compiled size limit,
    // so no call is folded.
    FuncRegistration::new(helper)
        .with_volatility(kind == ParamKind::Regex)
        .set_into_module(
            module,
            move |ctx: NativeCallContext, value: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> {
                parse(&ctx, &kind, value)
            },
        );
}

/// Register the parser of a kind taking an optional `range`, with and without
/// it.
fn register_ranged_parser(
    module: &mut rhai::Module,
    helper: &str,
    kind: fn(Option<IntRange>) -> ParamKind,
) {
    register_parser(module, helper, kind(None));
    FuncRegistration::new(helper).set_into_module(
        module,
        move |ctx: NativeCallContext,
              value: Dynamic,
              range: Dynamic|
              -> Result<Dynamic, Box<EvalAltResult>> {
            let range = take_range("range", range)?;
            parse(&ctx, &kind(Some(range)), value)
        },
    );
}

// spec: REX#compiled-size
fn parse(
    ctx: &NativeCallContext,
    kind: &ParamKind,
    value: Dynamic,
) -> Result<Dynamic, Box<EvalAltResult>> {
    let value = take_string("value", value)?;
    Ok(with_evaluation_budget(ctx, |budget| kind.parse(&value, budget))??.into_dynamic())
}

/// The name of the helper that parses the kind `name`.
// spec: ACN#kind-parsers
pub(crate) fn helper_name(name: ParamKindName) -> String {
    format!("parse_{}", name.as_str().replace('-', "_"))
}
