//! YAML, written by saphyr's emitter and read from saphyr's parser.

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
};

use rhai::EvalAltResult;
use saphyr::{Mapping, Scalar, ScalarStyle, Yaml, YamlEmitter};
use saphyr_parser::{Event, Parser, Span, StrInput, Tag};

use super::{Holds, MAX_DEPTH, Tree, not_a_document};

// spec: FMT#values-each-format-holds
pub(super) const HOLDS: Holds = Holds {
    name: "YAML",
    unit: true,
    non_finite: true,
    nested: true,
};

// spec: FMT#yaml-documents
/// The most values that aliases may copy in one document.
const MAX_ALIAS_COPIES: usize = 100_000;

// spec: FMT#yaml
pub(super) fn encode(tree: &Tree) -> String {
    let mut text = String::new();
    YamlEmitter::new(&mut text)
        .dump(&to_yaml(tree))
        .expect("writing YAML to a string does not fail");
    // The emitter starts every document with an explicit `---`.
    let mut text = text
        .strip_prefix("---\n")
        .map(str::to_owned)
        .unwrap_or(text);
    // spec: FMT#layout
    text.push('\n');
    text
}

fn to_yaml(tree: &Tree) -> Yaml<'static> {
    match tree {
        Tree::Unit => Yaml::Representation("null".into(), ScalarStyle::Plain, None),
        Tree::Bool(b) => Yaml::Value(Scalar::Boolean(*b)),
        Tree::Int(i) => Yaml::Value(Scalar::Integer(*i)),
        Tree::Float(f) => Yaml::Value(Scalar::FloatingPoint((*f).into())),
        Tree::Str(s) => string(s),
        Tree::Array(items) => Yaml::Sequence(items.iter().map(to_yaml).collect()),
        // spec: FMT#key-order
        Tree::Map(entries) => {
            let mut mapping = Mapping::new();
            for (key, value) in entries {
                mapping.insert(string(key), to_yaml(value));
            }
            Yaml::Mapping(mapping)
        }
    }
}

// spec: FMT#yaml
/// A string, left to the emitter to quote unless it holds a character the
/// emitter would write bare where no plain scalar may hold it. Those strings
/// are written double-quoted here, every such character escaped.
fn string(s: &str) -> Yaml<'static> {
    if !s.chars().any(unprintable) {
        return Yaml::Value(Scalar::String(s.to_owned().into()));
    }
    let mut quoted = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            c if c < ' ' || c == '\u{7f}' || unprintable(c) => {
                write!(quoted, "\\u{:04x}", u32::from(c))
                    .expect("writing to a string does not fail");
            }
            c => quoted.push(c),
        }
    }
    Yaml::Representation(quoted.into(), ScalarStyle::DoubleQuoted, None)
}

/// Outside YAML's printable set, and passed through by the emitter: C1
/// controls and the two noncharacters at the end of the BMP. The emitter
/// escapes the C0 controls and DEL itself.
fn unprintable(c: char) -> bool {
    matches!(c, '\u{80}'..='\u{9f}' | '\u{fffe}' | '\u{ffff}')
}

// spec: FMT#yaml-documents
pub(super) fn decode(text: &str) -> Result<Tree, Box<EvalAltResult>> {
    Loader {
        events: Parser::new_from_str(text),
        anchors: HashMap::new(),
        copies: 0,
    }
    .stream()
}

/// A node an anchor names, for its aliases to copy.
struct Anchored {
    tree: Tree,
    /// The values within it, itself included.
    values: usize,
    /// The levels of sequences and mappings within it, itself included.
    height: usize,
}

/// A decoded node and what an anchor on it records.
struct Loaded {
    tree: Tree,
    values: usize,
    height: usize,
}

struct Loader<'i> {
    events: Parser<'i, StrInput<'i>>,
    anchors: HashMap<usize, Anchored>,
    /// The values aliases have copied so far.
    copies: usize,
}

impl<'i> Loader<'i> {
    fn next(&mut self) -> Result<(Event<'i>, Span), Box<EvalAltResult>> {
        match self.events.next() {
            Some(Ok(event)) => Ok(event),
            Some(Err(e)) => Err(not_a_document("YAML", e.marker().line())),
            None => Err("the YAML parser stopped before the end of `text`".into()),
        }
    }

    // spec: FMT#yaml-documents
    /// The one document the stream holds.
    fn stream(mut self) -> Result<Tree, Box<EvalAltResult>> {
        let mut document = None;
        loop {
            let (event, span) = self.next()?;
            match event {
                Event::StreamStart => {}
                Event::StreamEnd => break,
                Event::DocumentStart(_) if document.is_some() => {
                    return Err(failure("more than one document", span));
                }
                Event::DocumentStart(_) => {
                    let (event, span) = self.next()?;
                    document = Some(self.node(event, span, 0)?.tree);
                }
                Event::DocumentEnd => {}
                _ => return Err(not_a_document("YAML", span.start.line())),
            }
        }
        document.ok_or_else(|| "`text` holds no YAML document".into())
    }

    /// The node `event` starts, within `levels` sequences and mappings.
    fn node(
        &mut self,
        event: Event<'i>,
        span: Span,
        levels: usize,
    ) -> Result<Loaded, Box<EvalAltResult>> {
        let (loaded, anchor) = match event {
            Event::Scalar(value, style, anchor, tag) => {
                let tree = scalar(&value, style, tag.as_deref(), span)?;
                (
                    Loaded {
                        tree,
                        values: 1,
                        height: 0,
                    },
                    anchor,
                )
            }
            Event::SequenceStart(anchor, tag) => {
                collection_tag(tag.as_deref(), "seq", span)?;
                let levels = nested(levels, span)?;
                let mut items = Vec::new();
                let (mut values, mut height) = (1, 1);
                loop {
                    let (event, span) = self.next()?;
                    if matches!(event, Event::SequenceEnd) {
                        break;
                    }
                    let item = self.node(event, span, levels)?;
                    values += item.values;
                    height = height.max(item.height + 1);
                    items.push(item.tree);
                }
                let tree = Tree::Array(items);
                (
                    Loaded {
                        tree,
                        values,
                        height,
                    },
                    anchor,
                )
            }
            Event::MappingStart(anchor, tag) => {
                collection_tag(tag.as_deref(), "map", span)?;
                let levels = nested(levels, span)?;
                let mut keys = HashSet::new();
                let mut entries = Vec::new();
                let (mut values, mut height) = (1, 1);
                loop {
                    let (event, key_span) = self.next()?;
                    if matches!(event, Event::MappingEnd) {
                        break;
                    }
                    let key = self.node(event, key_span, levels)?;
                    // spec: FMT#yaml-documents
                    let Tree::Str(key_text) = key.tree else {
                        return Err(failure("a mapping key that is not a string", key_span));
                    };
                    // spec: FMT#decoding
                    if !keys.insert(key_text.clone()) {
                        return Err(failure("the same key twice in one mapping", key_span));
                    }
                    let (event, span) = self.next()?;
                    let value = self.node(event, span, levels)?;
                    values += key.values + value.values;
                    height = height.max(value.height + 1);
                    entries.push((key_text, value.tree));
                }
                let tree = Tree::Map(entries);
                (
                    Loaded {
                        tree,
                        values,
                        height,
                    },
                    anchor,
                )
            }
            // spec: FMT#yaml-documents
            Event::Alias(id) => {
                let Some(anchored) = self.anchors.get(&id) else {
                    return Err(failure("an alias to no anchor", span));
                };
                self.copies += anchored.values;
                if self.copies > MAX_ALIAS_COPIES {
                    return Err(failure(
                        &format!("aliases copying more than {MAX_ALIAS_COPIES} values"),
                        span,
                    ));
                }
                // spec: FMT#helpers
                if levels + anchored.height > MAX_DEPTH {
                    return Err(depth(span));
                }
                let loaded = Loaded {
                    tree: anchored.tree.clone(),
                    values: anchored.values,
                    height: anchored.height,
                };
                return Ok(loaded);
            }
            _ => return Err(not_a_document("YAML", span.start.line())),
        };
        if anchor != 0 {
            self.anchors.insert(
                anchor,
                Anchored {
                    tree: loaded.tree.clone(),
                    values: loaded.values,
                    height: loaded.height,
                },
            );
        }
        Ok(loaded)
    }
}

/// A failure at `span`, describing what the text holds but never quoting it.
fn failure(what: &str, span: Span) -> Box<EvalAltResult> {
    format!("`text` holds {what} at line {}", span.start.line()).into()
}

// spec: FMT#helpers
fn nested(levels: usize, span: Span) -> Result<usize, Box<EvalAltResult>> {
    if levels + 1 > MAX_DEPTH {
        return Err(depth(span));
    }
    Ok(levels + 1)
}

fn depth(span: Span) -> Box<EvalAltResult> {
    failure(
        &format!("sequences and mappings nested more than {MAX_DEPTH} levels deep"),
        span,
    )
}

/// The suffix of a tag the core schema defines, or a script error.
// spec: FMT#yaml-documents
fn core_tag(tag: &Tag, span: Span) -> Result<&str, Box<EvalAltResult>> {
    match tag.suffix.as_str() {
        suffix @ ("str" | "int" | "float" | "bool" | "null" | "seq" | "map")
            if tag.is_yaml_core_schema() =>
        {
            Ok(suffix)
        }
        _ => Err(failure("a tag the core schema does not define", span)),
    }
}

// spec: FMT#yaml-documents
fn collection_tag(tag: Option<&Tag>, kind: &str, span: Span) -> Result<(), Box<EvalAltResult>> {
    match tag {
        Some(tag) if core_tag(tag, span)? != kind => {
            Err(failure("a node that is not of its tag's type", span))
        }
        _ => Ok(()),
    }
}

// spec: FMT#yaml-documents
// spec: FMT#decoding
/// A scalar as the core schema reads it. A quoted or block scalar with no
/// tag is a string; a tagged one must read as its tag's type.
fn scalar(
    value: &str,
    style: ScalarStyle,
    tag: Option<&Tag>,
    span: Span,
) -> Result<Tree, Box<EvalAltResult>> {
    let resolved = match tag {
        Some(tag) => match core_tag(tag, span)? {
            "str" => Tree::Str(value.to_string()),
            // The float form also matches decimal integers: `!!float 1` is the
            // float 1.0.
            "float" if is_core_float(value) => float(value, span)?,
            suffix => match resolve(value, span)? {
                Some(tree) if kind(&tree) == suffix => tree,
                _ => return Err(failure("a node that is not of its tag's type", span)),
            },
        },
        None if style == ScalarStyle::Plain => {
            resolve(value, span)?.unwrap_or_else(|| Tree::Str(value.to_string()))
        }
        None => Tree::Str(value.to_string()),
    };
    Ok(resolved)
}

fn kind(tree: &Tree) -> &'static str {
    match tree {
        Tree::Unit => "null",
        Tree::Bool(_) => "bool",
        Tree::Int(_) => "int",
        Tree::Float(_) => "float",
        Tree::Str(_) | Tree::Array(_) | Tree::Map(_) => "str",
    }
}

// spec: FMT#yaml-documents
/// The null, boolean, integer or float the core schema reads a plain scalar
/// as, or none for a string.
fn resolve(v: &str, span: Span) -> Result<Option<Tree>, Box<EvalAltResult>> {
    let tree = match v {
        "" | "~" | "null" | "Null" | "NULL" => Tree::Unit,
        "true" | "True" | "TRUE" => Tree::Bool(true),
        "false" | "False" | "FALSE" => Tree::Bool(false),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => Tree::Float(f64::INFINITY),
        "-.inf" | "-.Inf" | "-.INF" => Tree::Float(f64::NEG_INFINITY),
        ".nan" | ".NaN" | ".NAN" => Tree::Float(f64::NAN),
        _ => {
            let integer = if let Some(digits) = v.strip_prefix("0o") {
                is_digits(digits, 8).then(|| i64::from_str_radix(digits, 8))
            } else if let Some(digits) = v.strip_prefix("0x") {
                is_digits(digits, 16).then(|| i64::from_str_radix(digits, 16))
            } else {
                let unsigned = v.strip_prefix(['-', '+']).unwrap_or(v);
                is_digits(unsigned, 10).then(|| v.parse::<i64>())
            };
            if let Some(integer) = integer {
                return integer.map(|i| Some(Tree::Int(i))).map_err(|_| {
                    failure(
                        "an integer beyond the range of a 64-bit signed integer",
                        span,
                    )
                });
            }
            if !is_core_float(v) {
                return Ok(None);
            }
            float(v, span)?
        }
    };
    Ok(Some(tree))
}

// spec: FMT#decoding
/// A number in the core schema's float form, which must be finite.
fn float(v: &str, span: Span) -> Result<Tree, Box<EvalAltResult>> {
    let f: f64 = v
        .parse()
        .expect("Rust reads every number the core schema's float form matches");
    if !f.is_finite() {
        return Err(failure(
            "a number beyond the range of a floating-point number",
            span,
        ));
    }
    Ok(Tree::Float(f))
}

fn is_digits(s: &str, radix: u32) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_digit(radix))
}

/// Whether `v` matches the core schema's float form,
/// `[-+]? ( \. [0-9]+ | [0-9]+ ( \. [0-9]* )? ) ( [eE] [-+]? [0-9]+ )?`.
fn is_core_float(v: &str) -> bool {
    let v = v.strip_prefix(['-', '+']).unwrap_or(v);
    let (mantissa, exponent) = match v.find(['e', 'E']) {
        Some(at) => (&v[..at], Some(&v[at + 1..])),
        None => (v, None),
    };
    let mantissa_ok = match mantissa.split_once('.') {
        Some(("", fraction)) => is_digits(fraction, 10),
        Some((whole, fraction)) => {
            is_digits(whole, 10) && (fraction.is_empty() || is_digits(fraction, 10))
        }
        None => is_digits(mantissa, 10),
    };
    let exponent_ok =
        exponent.is_none_or(|e| is_digits(e.strip_prefix(['-', '+']).unwrap_or(e), 10));
    mantissa_ok && exponent_ok
}
