//! A parser for every parameter kind, `std::parse_<kind>`.

use rhai::{Dynamic, EvalAltResult, FuncRegistration};
use strum::VariantArray;

use crate::defs::{
    param_def::{ParamKind, ParsedValue},
    take::take_string,
};

// spec: ACN#kind-parsers
pub(super) fn register(module: &mut rhai::Module) {
    for &kind in ParamKind::VARIANTS {
        FuncRegistration::new(helper_name(kind)).set_into_module(
            module,
            move |value: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> {
                let value = take_string("value", value)?;
                Ok(match kind.parse(&value)? {
                    ParsedValue::Value(value) => value.into(),
                    ParsedValue::Email { local, domain } => {
                        let mut parts = rhai::Map::new();
                        parts.insert("local".into(), local.into());
                        parts.insert("domain".into(), domain.into());
                        parts.into()
                    }
                    ParsedValue::Version(version) => Dynamic::from(version),
                    ParsedValue::SemVer(version) => Dynamic::from(version),
                })
            },
        );
    }
}

/// The name of the helper that parses `kind`.
// spec: ACN#kind-parsers
pub(crate) fn helper_name(kind: ParamKind) -> String {
    format!("parse_{}", kind.as_str().replace('-', "_"))
}
