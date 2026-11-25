//! JSON, on serde_json.

use std::{cell::RefCell, collections::HashSet, fmt};

use rhai::EvalAltResult;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

use super::{Holds, MAX_DEPTH, Tree};

// spec: FMT#values-each-format-holds
pub(super) const HOLDS: Holds = Holds {
    name: "JSON",
    unit: true,
    non_finite: false,
    nested: true,
};

// spec: FMT#json
pub(super) fn encode(tree: &Tree) -> String {
    let mut text =
        serde_json::to_string_pretty(&to_value(tree)).expect("a JSON value always serialises");
    // spec: FMT#layout
    text.push('\n');
    text
}

fn to_value(tree: &Tree) -> Value {
    match tree {
        Tree::Unit => Value::Null,
        Tree::Bool(b) => Value::Bool(*b),
        Tree::Int(i) => Value::from(*i),
        Tree::Float(f) => Value::Number(
            serde_json::Number::from_f64(*f).expect("lowering refuses non-finite numbers"),
        ),
        Tree::Str(s) => Value::String(s.clone()),
        Tree::Array(items) => Value::Array(items.iter().map(to_value).collect()),
        // spec: FMT#key-order
        // The entries are already in key order, which the object keeps.
        Tree::Map(entries) => Value::Object(
            entries
                .iter()
                .map(|(k, v)| (k.clone(), to_value(v)))
                .collect(),
        ),
    }
}

// spec: FMT#decoding
pub(super) fn decode(text: &str) -> Result<Tree, Box<EvalAltResult>> {
    let numbers = RefCell::new(number_tokens(text).into_iter());
    let mut de = serde_json::Deserializer::from_str(text);
    let tree = Node {
        numbers: &numbers,
        levels: 0,
    }
    .deserialize(&mut de)
    .and_then(|tree| de.end().map(|()| tree))
    .map_err(failure)?;
    Ok(tree)
}

/// A decoding failure, naming the line it was found at. serde_json's own
/// messages, and the visitor's below, never quote the text.
fn failure(e: serde_json::Error) -> Box<EvalAltResult> {
    let message = e.to_string();
    let reason = message
        .rsplit_once(" at line ")
        .map_or(message.as_str(), |(reason, _)| reason);
    match e.classify() {
        serde_json::error::Category::Data => {
            format!("`text` holds {reason} at line {}", e.line()).into()
        }
        _ => format!("`text` is not JSON at line {}: {reason}", e.line()).into(),
    }
}

/// The text of each number in `text`, in the order it gives them, which is
/// the order serde_json visits them in.
///
/// serde_json reads an integer too large for 64 bits as a float, and `-0` as
/// one too, so the visitor classifies each number by its text instead. The
/// scan is only relied on up to where serde_json finds the text valid, where
/// a number starts wherever a minus sign or a digit stands outside a string.
fn number_tokens(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'-' | b'0'..=b'9' => {
                let start = i;
                while i < bytes.len()
                    && matches!(bytes[i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
                {
                    i += 1;
                }
                tokens.push(&text[start..i]);
            }
            _ => i += 1,
        }
    }
    tokens
}

/// A value at some depth of the document.
struct Node<'a, 't> {
    numbers: &'a RefCell<std::vec::IntoIter<&'t str>>,
    /// The arrays and objects the value sits within.
    levels: usize,
}

impl<'a, 't> Node<'a, 't> {
    fn within(&self, levels: usize) -> Node<'a, 't> {
        Node {
            numbers: self.numbers,
            levels,
        }
    }

    // spec: FMT#helpers
    fn nested<E: de::Error>(&self) -> Result<usize, E> {
        let levels = self.levels + 1;
        if levels > MAX_DEPTH {
            return Err(E::custom(format_args!(
                "arrays and objects nested more than {MAX_DEPTH} levels deep"
            )));
        }
        Ok(levels)
    }

    // spec: FMT#decoding
    fn number<E: de::Error>(self) -> Result<Tree, E> {
        let token = self
            .numbers
            .borrow_mut()
            .next()
            .ok_or_else(|| E::custom("a number the scan for numbers missed"))?;
        if token.contains(['.', 'e', 'E']) {
            let f: f64 = token
                .parse()
                .map_err(|_| E::custom("a number the scan for numbers misread"))?;
            if !f.is_finite() {
                return Err(E::custom(
                    "a number beyond the range of a floating-point number",
                ));
            }
            return Ok(Tree::Float(f));
        }
        token
            .parse()
            .map(Tree::Int)
            .map_err(|_| E::custom("an integer beyond the range of a 64-bit signed integer"))
    }
}

impl<'de> DeserializeSeed<'de> for Node<'_, '_> {
    type Value = Tree;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Tree, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Node<'_, '_> {
    type Value = Tree;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Tree, E> {
        Ok(Tree::Unit)
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Tree, E> {
        Ok(Tree::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Tree, E> {
        self.number()
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Tree, E> {
        self.number()
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Tree, E> {
        self.number()
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Tree, E> {
        Ok(Tree::Str(v.to_owned()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Tree, E> {
        Ok(Tree::Str(v))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Tree, A::Error> {
        let levels = self.nested()?;
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(self.within(levels))? {
            items.push(item);
        }
        Ok(Tree::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Tree, A::Error> {
        let levels = self.nested()?;
        let mut keys = HashSet::new();
        let mut entries = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            // spec: FMT#decoding
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom("the same key twice in one object"));
            }
            let value = map.next_value_seed(self.within(levels))?;
            entries.push((key, value));
        }
        Ok(Tree::Map(entries))
    }
}
