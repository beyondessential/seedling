//! JSON, written by serde_json and read by hifijson, which gives each
//! number's own text.

use std::{borrow::Cow, collections::HashSet};

use hifijson::{
    SliceLexer,
    token::Lex,
    value::{self, Value},
};
use rhai::EvalAltResult;

use super::{Holds, MAX_DEPTH, Tree, line_of, not_a_document};

// spec: FMT#values-each-format-holds
pub(super) const HOLDS: Holds = Holds {
    name: "JSON",
    unit: true,
    non_finite: false,
    nested: true,
};

// spec: FMT#json
pub(super) fn encode(tree: &Tree) -> String {
    let mut text = serde_json::to_string_pretty(tree)
        .expect("lowering refuses every value JSON has no form for");
    // spec: FMT#layout
    text.push('\n');
    text
}

// spec: FMT#decoding
pub(super) fn decode(text: &str) -> Result<Tree, Box<EvalAltResult>> {
    let mut lexer = SliceLexer::new(text.as_bytes());
    // A scalar within the deepest array or object allowed sits one level
    // below it.
    let parsed = lexer.exactly_one(Lex::ws_peek, |next, lexer| {
        value::parse_bounded(MAX_DEPTH + 1, next, lexer)
    });
    match parsed {
        Ok(value) => from_value(value),
        // spec: FMT#helpers
        Err(hifijson::Error::Depth) => {
            Err(format!("`text` nests arrays and objects more than {MAX_DEPTH} levels deep").into())
        }
        Err(_) => {
            let at = text.len() - lexer.as_slice().len();
            Err(not_a_document("JSON", line_of(text.as_bytes(), at)))
        }
    }
}

fn from_value(value: Value<&str, Cow<'_, str>>) -> Result<Tree, Box<EvalAltResult>> {
    Ok(match value {
        Value::Null => Tree::Unit,
        Value::Bool(b) => Tree::Bool(b),
        // spec: FMT#decoding
        Value::Number((text, parts)) if parts.is_int() => {
            Tree::Int(text.parse().map_err(|_| -> Box<EvalAltResult> {
                "`text` holds an integer beyond the range of a 64-bit signed integer".into()
            })?)
        }
        Value::Number((text, _)) => {
            let f: f64 = text
                .parse()
                .expect("Rust reads every number JSON's grammar allows");
            if !f.is_finite() {
                return Err(
                    "`text` holds a number beyond the range of a floating-point number".into(),
                );
            }
            Tree::Float(f)
        }
        Value::String(s) => Tree::Str(s.into_owned()),
        Value::Array(items) => Tree::Array(
            items
                .into_iter()
                .map(from_value)
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(members) => {
            let mut keys = HashSet::new();
            let mut entries = Vec::with_capacity(members.len());
            for (key, value) in members {
                let key = key.into_owned();
                // spec: FMT#decoding
                if !keys.insert(key.clone()) {
                    return Err("`text` holds the same key twice in one object".into());
                }
                entries.push((key, from_value(value)?));
            }
            Tree::Map(entries)
        }
    })
}
