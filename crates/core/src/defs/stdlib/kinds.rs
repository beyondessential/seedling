//! A parser for every parameter kind, `std::parse_<kind>`.

use rhai::{Dynamic, EvalAltResult, FuncRegistration};
use strum::VariantArray;

use crate::defs::{
    param_def::{ParamKind, ParamKindName, take_options, take_range},
    take::take_string,
};

// spec: ACN#kind-parsers
pub(super) fn register(module: &mut rhai::Module) {
    for &name in ParamKindName::VARIANTS {
        let helper = helper_name(name);
        match name {
            // spec: ACN#kind-parsers
            ParamKindName::Integer => {
                register_parser(module, &helper, ParamKind::Integer { range: None });
                FuncRegistration::new(&helper).set_into_module(
                    module,
                    |value: Dynamic, range: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> {
                        let range = take_range("range", range)?;
                        parse(&ParamKind::Integer { range: Some(range) }, value)
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
                    |value: Dynamic, options: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> {
                        let options = take_options("options", options)?;
                        parse(&ParamKind::Choice { options }, value)
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

fn parse(kind: &ParamKind, value: Dynamic) -> Result<Dynamic, Box<EvalAltResult>> {
    let value = take_string("value", value)?;
    Ok(kind.parse(&value)?.into_dynamic())
}

/// The name of the helper that parses the kind `name`.
// spec: ACN#kind-parsers
pub(crate) fn helper_name(name: ParamKindName) -> String {
    format!("parse_{}", name.as_str().replace('-', "_"))
}
