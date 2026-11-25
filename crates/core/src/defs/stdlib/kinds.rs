//! A parser for every parameter kind, `std::parse_<kind>`.

use rhai::{Dynamic, EvalAltResult, FuncRegistration};
use strum::VariantArray;

use crate::defs::{
    ip::take_family,
    param_def::{IntRange, ParamKind, ParamKindName, take_options, take_range},
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
                    |value: Dynamic, options: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> {
                        let options = take_options("options", options)?;
                        parse(&ParamKind::Choice { options }, value)
                    },
                );
            }
            // spec: IP#family-argument
            ParamKindName::IpAddress | ParamKindName::IpSubnet | ParamKindName::IpRange => {
                let kind = ParamKind::without_arguments(name).expect("family is optional");
                register_parser(module, &helper, kind);
                FuncRegistration::new(&helper).set_into_module(
                    module,
                    move |value: Dynamic, family: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> {
                        let family = take_family("family", family)?;
                        let kind = ParamKind::of_family(name, family).expect("an IP kind");
                        parse(&kind, value)
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
    FuncRegistration::new(helper).set_into_module(
        module,
        move |value: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> { parse(&kind, value) },
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
        move |value: Dynamic, range: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> {
            let range = take_range("range", range)?;
            parse(&kind(Some(range)), value)
        },
    );
}

fn parse(kind: &ParamKind, value: Dynamic) -> Result<Dynamic, Box<EvalAltResult>> {
    let value = take_string("value", value)?;
    Ok(kind.parse(&value)?.into_dynamic())
}

/// The name of the helper that parses the kind `name`.
// spec: ACN#kind-parsers
pub(crate) fn helper_name(name: ParamKindName) -> String {
    format!("parse_{}", name.as_str().replace('-', "_"))
}
