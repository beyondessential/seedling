//! Structured formats: helpers that write a value as JSON, YAML, TOML or
//! env-file text, and read one back from such text.

use std::{collections::BTreeSet, fmt};

use rhai::{Dynamic, EvalAltResult, FuncRegistration, NativeCallContext};

use super::{
    ARRAY, MAP, STRING,
    bytes::{ByteArg, held, take_byte_arg},
    max_string, settle, within,
};
use crate::defs::{
    take::take_map,
    version::{SemVer, Version},
};

mod env;
mod json;
mod toml;
mod yaml;

type HelperResult = Result<Dynamic, Box<EvalAltResult>>;

// spec: FMT#helpers
/// The most levels of arrays and maps a value may nest, counting the
/// outermost array or map as the first.
const MAX_DEPTH: usize = 64;

pub(super) fn register(module: &mut rhai::Module) {
    // spec: FMT#helpers
    FuncRegistration::new("encode_json").set_into_module(
        module,
        |ctx: NativeCallContext, value: Dynamic| -> HelperResult {
            let tree = json::HOLDS.lower(value)?;
            encoded(&ctx, &tree, json::encode(&tree))
        },
    );
    // spec: FMT#helpers
    FuncRegistration::new("decode_json").set_into_module(
        module,
        |ctx: NativeCallContext, text: Dynamic| -> HelperResult {
            let text = take_byte_arg("text", text)?;
            let tree = json::decode(document(&text)?)?;
            decoded(&ctx, &text, tree)
        },
    );
    // spec: FMT#helpers
    FuncRegistration::new("encode_yaml").set_into_module(
        module,
        |ctx: NativeCallContext, value: Dynamic| -> HelperResult {
            let tree = yaml::HOLDS.lower(value)?;
            encoded(&ctx, &tree, yaml::encode(&tree))
        },
    );
    // spec: FMT#helpers
    FuncRegistration::new("decode_yaml").set_into_module(
        module,
        |ctx: NativeCallContext, text: Dynamic| -> HelperResult {
            let text = take_byte_arg("text", text)?;
            let tree = yaml::decode(document(&text)?)?;
            decoded(&ctx, &text, tree)
        },
    );
    // spec: FMT#helpers
    FuncRegistration::new("encode_toml").set_into_module(
        module,
        |ctx: NativeCallContext, value: Dynamic| -> HelperResult {
            let tree = toml::HOLDS.lower_map(take_map("value", value)?)?;
            encoded(&ctx, &tree, toml::encode(&tree))
        },
    );
    // spec: FMT#helpers
    FuncRegistration::new("decode_toml").set_into_module(
        module,
        |ctx: NativeCallContext, text: Dynamic| -> HelperResult {
            let text = take_byte_arg("text", text)?;
            let tree = toml::decode(document(&text)?)?;
            decoded(&ctx, &text, tree)
        },
    );
    // spec: FMT#helpers
    FuncRegistration::new("encode_env").set_into_module(
        module,
        |ctx: NativeCallContext, value: Dynamic| -> HelperResult {
            let tree = env::HOLDS.lower_map(take_map("value", value)?)?;
            let text = env::encode(&tree)?;
            encoded(&ctx, &tree, text)
        },
    );
    // spec: FMT#helpers
    FuncRegistration::new("decode_env").set_into_module(
        module,
        |ctx: NativeCallContext, text: Dynamic| -> HelperResult {
            let text = take_byte_arg("text", text)?;
            let tree = env::decode(document(&text)?)?;
            decoded(&ctx, &text, tree)
        },
    );
}

/// A value as a structured format holds it, between a BSL value and the
/// format's text.
#[derive(Clone, Debug, PartialEq)]
enum Tree {
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<Tree>),
    /// Entries in ascending bytewise order of their keys when lowered from a
    /// BSL value, and in the order the text gives them when decoded.
    Map(Vec<(String, Tree)>),
}

impl Tree {
    /// Every string within the value, map keys included.
    fn strings<'a>(&'a self, out: &mut BTreeSet<&'a str>) {
        match self {
            Tree::Str(s) => {
                out.insert(s);
            }
            Tree::Array(items) => items.iter().for_each(|item| item.strings(out)),
            Tree::Map(entries) => {
                for (key, value) in entries {
                    out.insert(key);
                    value.strings(out);
                }
            }
            Tree::Unit | Tree::Bool(_) | Tree::Int(_) | Tree::Float(_) => {}
        }
    }

    fn into_dynamic(self) -> Dynamic {
        match self {
            Tree::Unit => Dynamic::UNIT,
            Tree::Bool(b) => b.into(),
            Tree::Int(i) => i.into(),
            Tree::Float(f) => f.into(),
            Tree::Str(s) => s.into(),
            Tree::Array(items) => {
                Dynamic::from_array(items.into_iter().map(Tree::into_dynamic).collect())
            }
            Tree::Map(entries) => Dynamic::from_map(
                entries
                    .into_iter()
                    .map(|(k, v)| (k.into(), v.into_dynamic()))
                    .collect(),
            ),
        }
    }
}

// spec: FMT#values-each-format-holds
/// The values a format holds, beyond the booleans, integers, finite numbers
/// and strings every format does.
struct Holds {
    /// The format's name in a reason, such as "TOML has no null".
    name: &'static str,
    unit: bool,
    non_finite: bool,
    /// Arrays, and maps within the argument.
    nested: bool,
}

impl Holds {
    /// The value as this format holds it, or a script error naming the
    /// position of the first value it refuses.
    fn lower(&self, value: Dynamic) -> Result<Tree, Box<EvalAltResult>> {
        self.lower_at(value, &mut Position::default(), 0)
    }

    /// A map argument as this format holds it.
    fn lower_map(&self, map: rhai::Map) -> Result<Tree, Box<EvalAltResult>> {
        self.lower_entries(map, &mut Position::default(), 1)
    }

    /// `value` at `at`, within `levels` arrays and maps.
    fn lower_at(
        &self,
        value: Dynamic,
        at: &mut Position,
        levels: usize,
    ) -> Result<Tree, Box<EvalAltResult>> {
        let type_name = value.type_name();
        // take: dispatch
        if value.is_unit() {
            return match self.unit {
                true => Ok(Tree::Unit),
                false => Err(at.refused(&format!("{} has no null", self.name))),
            };
        }
        // take: dispatch
        if value.is_bool() {
            return Ok(Tree::Bool(value.cast::<bool>()));
        }
        // take: dispatch
        if value.is_int() {
            return Ok(Tree::Int(value.cast::<rhai::INT>()));
        }
        // take: dispatch
        if value.is_float() {
            let f = value.cast::<rhai::FLOAT>();
            if !f.is_finite() && !self.non_finite {
                return Err(at.refused(&format!("{} has no infinite or NaN number", self.name)));
            }
            return Ok(Tree::Float(f));
        }
        // take: dispatch
        if value.is_string() {
            return Ok(Tree::Str(value.into_immutable_string()?.into_owned()));
        }
        // spec: FMT#values-each-format-holds
        // take: dispatch
        if let Some(version) = value.clone().try_cast::<Version>() {
            return Ok(Tree::Str(version.to_string()));
        }
        // take: dispatch
        if let Some(version) = value.clone().try_cast::<SemVer>() {
            return Ok(Tree::Str(version.to_string()));
        }
        // take: dispatch
        if value.is_array() {
            if !self.nested {
                return Err(at.refused(&format!("{} has no arrays", self.name)));
            }
            let levels = at.nested(levels)?;
            let mut items = Vec::new();
            for (i, item) in value.cast::<rhai::Array>().into_iter().enumerate() {
                at.0.push(Step::Index(i));
                items.push(self.lower_at(item, at, levels)?);
                at.0.pop();
            }
            return Ok(Tree::Array(items));
        }
        // take: dispatch
        if value.is_map() {
            if !self.nested {
                return Err(at.refused(&format!("{} has no nested maps", self.name)));
            }
            let levels = at.nested(levels)?;
            return self.lower_entries(value.cast::<rhai::Map>(), at, levels);
        }
        Err(at.refused(&format!("{} has no type for {type_name}", self.name)))
    }

    // spec: FMT#key-order
    fn lower_entries(
        &self,
        map: rhai::Map,
        at: &mut Position,
        levels: usize,
    ) -> Result<Tree, Box<EvalAltResult>> {
        let mut entries = Vec::with_capacity(map.len());
        for (key, value) in map {
            at.0.push(Step::Key(key.to_string()));
            entries.push((key.to_string(), self.lower_at(value, at, levels)?));
            at.0.pop();
        }
        entries.sort_by(|(a, _), (b, _)| a.as_bytes().cmp(b.as_bytes()));
        Ok(Tree::Map(entries))
    }
}

// spec: FMT#refusal
/// Where a value sits within an encoder's argument.
#[derive(Default)]
struct Position(Vec<Step>);

enum Step {
    Key(String),
    Index(usize),
}

impl Position {
    // spec: FMT#refusal
    fn refused(&self, reason: &str) -> Box<EvalAltResult> {
        format!("`{self}` cannot be encoded: {reason}").into()
    }

    /// The levels a collection at this position sits within, itself included.
    // spec: FMT#helpers
    fn nested(&self, levels: usize) -> Result<usize, Box<EvalAltResult>> {
        let levels = levels + 1;
        if levels > MAX_DEPTH {
            return Err(format!(
                "`{self}` nests arrays and maps more than {MAX_DEPTH} levels deep"
            )
            .into());
        }
        Ok(levels)
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("value")?;
        for step in &self.0 {
            match step {
                Step::Key(key) => write!(f, "[{key:?}]")?,
                Step::Index(i) => write!(f, "[{i}]")?,
            }
        }
        Ok(())
    }
}

/// The text of a finite floating-point number, as JSON writes it: with a
/// fraction or an exponent, and decoding to the same number.
fn float_text(f: f64) -> String {
    serde_json::Number::from_f64(f)
        .expect("only finite numbers have JSON text")
        .to_string()
}

// spec: FMT#layout
/// An encoder's result, withheld when any string in its argument contains a
/// withheld value.
fn encoded(ctx: &NativeCallContext, tree: &Tree, text: String) -> HelperResult {
    within(text.len(), max_string(ctx), STRING)?;
    let mut strings = BTreeSet::new();
    tree.strings(&mut strings);
    let held: Vec<&[u8]> = strings.iter().map(|s| s.as_bytes()).collect();
    settle(ctx, &held, || vec![vec![text.as_bytes().to_vec()]])?;
    Ok(text.into())
}

// spec: FMT#decoding
/// The text of a decoder's `text` argument, without a byte order mark.
fn document(text: &ByteArg) -> Result<&str, Box<EvalAltResult>> {
    let bytes = text
        .bytes
        .strip_prefix(b"\xef\xbb\xbf")
        .unwrap_or(&text.bytes);
    std::str::from_utf8(bytes).map_err(|e| {
        format!(
            "`text` is not valid UTF-8 at line {}",
            line_of(bytes, e.valid_up_to())
        )
        .into()
    })
}

/// The line, counted from 1, holding byte `at` of `text`.
fn line_of(text: &[u8], at: usize) -> usize {
    text[..at.min(text.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

// spec: FMT#decoding
/// A decoder's failure at `line`, naming where `text` stops being a document
/// of its format but never what it holds: it may be a secret.
fn not_a_document(format: &str, line: usize) -> Box<EvalAltResult> {
    format!("`text` is not {format} at line {line}").into()
}

/// A decoder's result, held to the engine limits and withheld string by
/// string when its argument contains a withheld value.
fn decoded(ctx: &NativeCallContext, text: &ByteArg, tree: Tree) -> HelperResult {
    // spec: FMT#helpers
    within_limits(ctx, &tree, 0)?;
    // spec: SEC#redaction
    // An empty string withholds nothing, and a string repeated is one value.
    let mut strings = BTreeSet::new();
    tree.strings(&mut strings);
    settle(ctx, &held(&[text]), || {
        strings
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| vec![s.as_bytes().to_vec()])
            .collect()
    })?;
    Ok(tree.into_dynamic())
}

// spec: FMT#helpers
// spec: LIM#limits
fn within_limits(
    ctx: &NativeCallContext,
    tree: &Tree,
    levels: usize,
) -> Result<(), Box<EvalAltResult>> {
    let engine = ctx.engine();
    let deeper = || -> Result<usize, Box<EvalAltResult>> {
        if levels + 1 > MAX_DEPTH {
            return Err(
                format!("`text` nests arrays and maps more than {MAX_DEPTH} levels deep").into(),
            );
        }
        Ok(levels + 1)
    };
    match tree {
        Tree::Str(s) => within(s.len(), engine.max_string_size(), STRING),
        Tree::Array(items) => {
            let levels = deeper()?;
            within(items.len(), engine.max_array_size(), ARRAY)?;
            items
                .iter()
                .try_for_each(|item| within_limits(ctx, item, levels))
        }
        Tree::Map(entries) => {
            let levels = deeper()?;
            within(entries.len(), engine.max_map_size(), MAP)?;
            entries.iter().try_for_each(|(key, value)| {
                within(key.len(), engine.max_string_size(), STRING)?;
                within_limits(ctx, value, levels)
            })
        }
        Tree::Unit | Tree::Bool(_) | Tree::Int(_) | Tree::Float(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests;
