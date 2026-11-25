//! TOML, written here as TOML 1.0.0 and read by the toml crate, which reads
//! TOML 1.1.0.

use std::fmt::Write as _;

use rhai::EvalAltResult;

use super::{Holds, Tree, float_text, line_of, not_a_document};

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
    let mut out = String::new();
    table(&mut out, &mut Vec::new(), entries);
    out
}

// spec: FMT#toml
/// Whether an array is written as an array of tables.
fn is_table_array(items: &[Tree]) -> bool {
    !items.is_empty() && items.iter().all(|item| matches!(item, Tree::Map(_)))
}

/// Whether an entry is written as a table rather than a key/value line.
fn is_table(value: &Tree) -> bool {
    match value {
        Tree::Map(_) => true,
        Tree::Array(items) => is_table_array(items),
        _ => false,
    }
}

// spec: FMT#toml
/// The entries of the table at `path`: its key/value lines, then its tables,
/// each in key order.
fn table(out: &mut String, path: &mut Vec<String>, entries: &[(String, Tree)]) {
    for (key, value) in entries.iter().filter(|(_, value)| !is_table(value)) {
        key_text(out, key);
        out.push_str(" = ");
        inline(out, value);
        out.push('\n');
    }
    for (key, value) in entries.iter().filter(|(_, value)| is_table(value)) {
        path.push(key.clone());
        match value {
            Tree::Map(entries) => {
                header(out, path, "[", "]");
                table(out, path, entries);
            }
            Tree::Array(items) => {
                for item in items {
                    let Tree::Map(entries) = item else {
                        unreachable!("an array of tables holds only maps")
                    };
                    header(out, path, "[[", "]]");
                    table(out, path, entries);
                }
            }
            _ => unreachable!("only maps and arrays of maps are tables"),
        }
        path.pop();
    }
}

fn header(out: &mut String, path: &[String], open: &str, close: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(open);
    for (i, key) in path.iter().enumerate() {
        if i > 0 {
            out.push('.');
        }
        key_text(out, key);
    }
    out.push_str(close);
    out.push('\n');
}

// spec: FMT#toml
fn key_text(out: &mut String, key: &str) {
    let bare = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if bare {
        out.push_str(key);
    } else {
        string(out, key);
    }
}

// spec: FMT#values-each-format-holds
fn inline(out: &mut String, value: &Tree) {
    match value {
        Tree::Unit => unreachable!("lowering refuses unit for TOML"),
        Tree::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Tree::Int(i) => write!(out, "{i}").expect("writing to a string does not fail"),
        Tree::Float(f) if f.is_nan() => out.push_str("nan"),
        Tree::Float(f) if f.is_infinite() => out.push_str(if *f > 0.0 { "inf" } else { "-inf" }),
        Tree::Float(f) => out.push_str(&float_text(*f)),
        Tree::Str(s) => string(out, s),
        Tree::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                inline(out, item);
            }
            out.push(']');
        }
        Tree::Map(entries) if entries.is_empty() => out.push_str("{}"),
        Tree::Map(entries) => {
            out.push_str("{ ");
            for (i, (key, value)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                key_text(out, key);
                out.push_str(" = ");
                inline(out, value);
            }
            out.push_str(" }");
        }
    }
}

/// A basic string, escaping only what TOML 1.0.0 requires escaped.
fn string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if c < ' ' || c == '\u{7f}' => {
                write!(out, "\\u{:04X}", u32::from(c)).expect("writing to a string does not fail");
            }
            c => out.push(c),
        }
    }
    out.push('"');
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
