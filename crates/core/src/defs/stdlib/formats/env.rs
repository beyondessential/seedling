//! Env files: one string variable per line, every value single-quoted when
//! written, and read in the strict subset that shells, systemd and the
//! common dotenv readers agree on.

use std::{borrow::Cow, collections::HashSet, fmt::Write as _};

use rhai::EvalAltResult;
use winnow::{
    Parser,
    combinator::{alt, delimited, eof, terminated},
    token::{one_of, take_while},
};

use super::{Holds, Position, Step, Tree, float_text, not_a_document};

// spec: FMT#values-each-format-holds
pub(super) const HOLDS: Holds = Holds {
    name: "an env file",
    unit: false,
    non_finite: false,
    nested: false,
};

// spec: FMT#env-files
pub(super) fn encode(tree: &Tree) -> Result<String, Box<EvalAltResult>> {
    let Tree::Map(entries) = tree else {
        unreachable!("encode_env lowers only a map")
    };
    let mut out = String::new();
    for (name, value) in entries {
        let at = || Position(vec![Step::Key(name.clone())]);
        if key.parse(name.as_str()).is_err() {
            return Err(at().refused(
                "an env file key is ASCII letters, digits and underscores, not beginning with a digit",
            ));
        }
        // spec: FMT#values-each-format-holds
        let text: Cow<'_, str> = match value {
            Tree::Bool(b) => b.to_string().into(),
            Tree::Int(i) => i.to_string().into(),
            Tree::Float(f) => float_text(*f).into(),
            Tree::Str(s) => s.as_str().into(),
            Tree::Unit | Tree::Array(_) | Tree::Map(_) => {
                unreachable!("lowering refuses these for an env file")
            }
        };
        if text.contains(['\'', '\n', '\r', '\0']) {
            return Err(at().refused(
                "an env file value cannot hold a single quote, a line feed, a carriage return or NUL",
            ));
        }
        writeln!(out, "{name}='{text}'").expect("writing to a string does not fail");
    }
    Ok(out)
}

// spec: FMT#env-files
// spec: FMT#decoding
pub(super) fn decode(text: &str) -> Result<Tree, Box<EvalAltResult>> {
    // The line feed ending the last line starts no line of its own.
    let text = text.strip_suffix('\n').unwrap_or(text);
    let mut names = HashSet::new();
    let mut entries = Vec::new();
    for (i, text) in text.split('\n').enumerate() {
        let Ok(parsed) = line.parse(text) else {
            return Err(not_a_document("an env file", i + 1));
        };
        if let Some((name, value)) = parsed {
            if !names.insert(name) {
                return Err(format!("`text` names the same key twice, at line {}", i + 1).into());
            }
            entries.push((name.to_owned(), Tree::Str(value.to_owned())));
        }
    }
    Ok(Tree::Map(entries))
}

/// A line: blank, a comment, or an entry, which gives its key and value.
fn line<'i>(input: &mut &'i str) -> winnow::Result<Option<(&'i str, &'i str)>> {
    alt((
        ('#', take_while(0.., |c| c != '\r'), eof).value(None),
        (terminated(key, '='), terminated(value, eof)).map(Some),
        (take_while(0.., [' ', '\t']), eof).value(None),
    ))
    .parse_next(input)
}

fn key<'i>(input: &mut &'i str) -> winnow::Result<&'i str> {
    (
        one_of(|c: char| c.is_ascii_alphabetic() || c == '_'),
        take_while(0.., |c: char| c.is_ascii_alphanumeric() || c == '_'),
    )
        .take()
        .parse_next(input)
}

/// A value: single-quoted, bare, or empty, giving its characters.
fn value<'i>(input: &mut &'i str) -> winnow::Result<&'i str> {
    alt((
        delimited(
            '\'',
            take_while(0.., |c| !matches!(c, '\'' | '\r' | '\0')),
            '\'',
        ),
        take_while(1.., |c: char| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | '-' | '.' | '/' | ':' | '@' | '%' | '+' | ',' | '=')
        }),
        "".value(""),
    ))
    .parse_next(input)
}
