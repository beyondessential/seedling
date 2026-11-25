//! TOML, written and read by the toml crate. It writes TOML 1.0.0 for the
//! values BSL has, and reads TOML 1.1.0.

use rhai::EvalAltResult;

use super::{Holds, Tree, line_of, not_a_document};

// spec: FMT#values-each-format-holds
// spec: FMT#toml
pub(super) const HOLDS: Holds = Holds {
    name: "TOML",
    unit: false,
    non_finite: true,
    nested: true,
};

// spec: FMT#toml
pub(super) fn encode(tree: &Tree) -> String {
    let Tree::Map(entries) = tree else {
        unreachable!("encode_toml lowers only a map")
    };
    ::toml::to_string(&to_table(entries)).expect("every value BSL lowers to has a TOML form")
}

// spec: FMT#key-order
// The entries are already in key order, which the table keeps whether it is
// ordered by insertion or by key.
fn to_table(entries: &[(String, Tree)]) -> ::toml::Table {
    entries
        .iter()
        .map(|(k, v)| (k.clone(), to_value(v)))
        .collect()
}

fn to_value(tree: &Tree) -> ::toml::Value {
    match tree {
        Tree::Unit => unreachable!("lowering refuses unit for TOML"),
        Tree::Bool(b) => ::toml::Value::Boolean(*b),
        Tree::Int(i) => ::toml::Value::Integer(*i),
        Tree::Float(f) => ::toml::Value::Float(*f),
        Tree::Str(s) => ::toml::Value::String(s.clone()),
        Tree::Array(items) => ::toml::Value::Array(items.iter().map(to_value).collect()),
        Tree::Map(entries) => ::toml::Value::Table(to_table(entries)),
    }
}

// spec: FMT#decoding
pub(super) fn decode(text: &str) -> Result<Tree, Box<EvalAltResult>> {
    let table = ::toml::from_str::<::toml::Table>(text).map_err(|e| {
        let at = e.span().map_or(0, |span| span.start);
        not_a_document("TOML", line_of(text.as_bytes(), at))
    })?;
    Ok(from_table(table))
}

fn from_table(table: ::toml::Table) -> Tree {
    Tree::Map(table.into_iter().map(|(k, v)| (k, from_value(v))).collect())
}

fn from_value(value: ::toml::Value) -> Tree {
    match value {
        ::toml::Value::String(s) => Tree::Str(s),
        ::toml::Value::Integer(i) => Tree::Int(i),
        ::toml::Value::Float(f) => Tree::Float(f),
        ::toml::Value::Boolean(b) => Tree::Bool(b),
        // spec: FMT#decoding
        ::toml::Value::Datetime(d) => Tree::Str(d.to_string()),
        ::toml::Value::Array(items) => Tree::Array(items.into_iter().map(from_value).collect()),
        ::toml::Value::Table(table) => from_table(table),
    }
}
