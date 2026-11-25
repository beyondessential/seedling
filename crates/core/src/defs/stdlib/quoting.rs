//! Quoting: helpers that quote a string so a program parsing text it is
//! placed in reads back the same string.

use rhai::{Dynamic, EvalAltResult};

use super::{bytes::ByteArg, text_result, unary};
use crate::defs::take::{take_string, take_string_array};

pub(super) fn register(module: &mut rhai::Module) {
    // spec: QUOT#shell-words
    unary(module, "quote_shell", |ctx, words| {
        let words = take_words(words)?;
        let quoted = words
            .iter()
            .map(|word| quote_shell(word))
            .collect::<Result<Vec<_>, _>>()?
            .join(" ");
        let args: Vec<ByteArg> = words.into_iter().map(ByteArg::text).collect();
        // spec: QUOT#results
        text_result(ctx, &args.iter().collect::<Vec<_>>(), quoted)
    });
}

// spec: QUOT#shell-words
/// A string, as one word, or an array of strings, as one word each.
fn take_words(words: Dynamic) -> Result<Vec<String>, Box<EvalAltResult>> {
    let type_name = words.type_name();
    // take: dispatch
    if words.is_string() {
        return Ok(vec![take_string("words", words)?]);
    }
    // take: dispatch
    if words.is_array() {
        return take_string_array("words", words);
    }
    Err(format!("`words` must be a string or an array of strings, got {type_name}").into())
}

/// The characters besides ASCII letters and digits that the shell gives no
/// meaning in any position of a word.
const UNQUOTED: &[u8] = b"_-./:,+@%";

// spec: QUOT#shell-words
fn quote_shell(word: &str) -> Result<String, Box<EvalAltResult>> {
    // spec: SEC#redaction
    if word.contains('\0') {
        return Err("`words` holds a NUL character, which no shell word can hold".into());
    }
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || UNQUOTED.contains(&b));
    if plain {
        return Ok(word.to_owned());
    }
    Ok(format!("'{}'", word.replace('\'', r"'\''")))
}

#[cfg(test)]
#[expect(
    clippy::disallowed_methods,
    reason = "tests evaluate snippets of BSL directly, not definitions"
)]
mod tests;
