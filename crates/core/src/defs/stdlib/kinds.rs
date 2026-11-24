//! A parser for every parameter kind, `std::parse_<kind>`.

use rhai::{Dynamic, EvalAltResult, FuncRegistration};
use strum::VariantArray;

use crate::defs::{
    param_def::{ParamKind, ParsedValue},
    purl::{Purl, QualifierValue},
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
                    ParsedValue::Purl(purl) => purl_map(purl),
                })
            },
        );
    }
}

/// A Package URL's components as a map, leaving out those it does not have.
// spec: PURL#components
fn purl_map(purl: Purl) -> Dynamic {
    let mut map = rhai::Map::new();
    map.insert("type".into(), purl.ty.into());
    if let Some(namespace) = purl.namespace {
        map.insert("namespace".into(), namespace.into());
    }
    map.insert("name".into(), purl.name.into());
    if let Some(version) = purl.version {
        map.insert("version".into(), Dynamic::from(version));
    }
    if !purl.qualifiers.is_empty() {
        let qualifiers: rhai::Map = purl
            .qualifiers
            .into_iter()
            .map(|(key, value)| {
                let value = match value {
                    QualifierValue::Text(text) => text.into(),
                    QualifierValue::Checksums(entries) => entries
                        .into_iter()
                        .map(Dynamic::from)
                        .collect::<rhai::Array>()
                        .into(),
                };
                (key.into(), value)
            })
            .collect();
        map.insert("qualifiers".into(), qualifiers.into());
    }
    if let Some(subpath) = purl.subpath {
        map.insert("subpath".into(), subpath.into());
    }
    map.into()
}

/// The name of the helper that parses `kind`.
// spec: ACN#kind-parsers
pub(crate) fn helper_name(kind: ParamKind) -> String {
    format!("parse_{}", kind.as_str().replace('-', "_"))
}
