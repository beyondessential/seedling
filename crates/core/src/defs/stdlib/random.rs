//! Derived random values: bytes, strings and UUIDs a script draws from a seed
//! the runtime holds, so the same call returns the same value wherever the
//! seed is the same.

use std::collections::HashSet;

use chacha20::{ChaCha20, KeyIvInit, cipher::StreamCipher};
use hmac::{Hmac, KeyInit, Mac};
use rhai::{Blob, Dynamic, EvalAltResult, FuncRegistration, INT, NativeCallContext};
use sha2::Sha256;

use super::{
    BLOB, STRING, blob_forms,
    fields::{Field, encode},
    helper_limits, too_large,
};
use crate::{
    defs::take::take_string,
    runtime::{
        barrier::runtime::is_in_action_closure,
        seeds::{self, Seed},
    },
};

const RANDOM_BYTES: &str = "random_bytes";
const RANDOM_STRING: &str = "random_string";
const UUID_V4: &str = "uuid_v4";

// spec: RND#helpers
pub(super) fn register(module: &mut rhai::Module) {
    // The value depends on the seed the calling context selects, which the
    // optimiser cannot see, so no call is ever folded into a constant.
    FuncRegistration::new(RANDOM_BYTES)
        .with_volatility(true)
        .set_into_module(
            module,
            |ctx: NativeCallContext, label: Dynamic, n: INT| -> Result<Blob, Box<EvalAltResult>> {
                let label = take_label(label)?;
                let n = take_count("n", n, ctx.engine().max_array_size(), BLOB)?;
                let seed = seed_for_call(&ctx)?;
                let bytes = random_bytes(&seed, &label, n)?;
                record(|| blob_forms(&bytes));
                Ok(bytes)
            },
        );

    FuncRegistration::new(RANDOM_STRING)
        .with_volatility(true)
        .set_into_module(
            module,
            |ctx: NativeCallContext,
             label: Dynamic,
             length: INT,
             alphabet: Dynamic|
             -> Result<String, Box<EvalAltResult>> {
                let label = take_label(label)?;
                let alphabet = take_alphabet(alphabet)?;
                // The engine limits a string in UTF-8 bytes, so the most
                // characters allowed are as many of the widest as fit.
                let length = take_count(
                    "length",
                    length,
                    ctx.engine().max_string_size() / widest(&alphabet),
                    STRING,
                )?;
                let seed = seed_for_call(&ctx)?;
                let value = random_string(&seed, &label, length, &alphabet)?;
                record(|| vec![value.clone()]);
                Ok(value)
            },
        );

    FuncRegistration::new(UUID_V4)
        .with_volatility(true)
        .set_into_module(
            module,
            |ctx: NativeCallContext, label: Dynamic| -> Result<String, Box<EvalAltResult>> {
                let label = take_label(label)?;
                let seed = seed_for_call(&ctx)?;
                let value = uuid_v4(&seed, &label)?;
                record(|| vec![value.clone()]);
                Ok(value)
            },
        );
}

// spec: RND#seeds
/// The seed the calling context derives from: the operation's inside an
/// action closure, the app's in the static and validator contexts.
// spec: LIM#limits
/// Counted against the evaluation's limit on derived values.
fn seed_for_call(ctx: &NativeCallContext) -> Result<Seed, Box<EvalAltResult>> {
    if !seeds::count_derivation(helper_limits(ctx)?.derived_values) {
        return Err(too_large("Number of derived values"));
    }
    let seed = if is_in_action_closure() {
        seeds::operation_seed()
    } else {
        seeds::app_seed()
    };
    seed.map_err(Into::into)
}

/// Record a value derived from the app seed. Values derived inside an action
/// closure come from the operation seed and are the operation's own.
fn record<F: AsRef<[u8]>>(forms: impl FnOnce() -> Vec<F>) {
    if !is_in_action_closure() {
        seeds::record_derived(forms());
    }
}

// spec: RND#helpers
fn take_label(label: Dynamic) -> Result<String, Box<EvalAltResult>> {
    let label = take_string("label", label)?;
    if label.is_empty() {
        return Err("`label` must not be empty".into());
    }
    Ok(label)
}

// spec: RND#helpers
/// A count of at least 1, refused before anything is allocated where it is
/// beyond the engine's limit for the value it sizes.
// spec: LIM#limits
fn take_count(
    what: &str,
    n: INT,
    limit: usize,
    limit_name: &str,
) -> Result<usize, Box<EvalAltResult>> {
    if n < 1 {
        return Err(format!("`{what}` must be at least 1, got {n}").into());
    }
    let n = usize::try_from(n).map_err(|_| too_large(limit_name))?;
    if limit != 0 && n > limit {
        return Err(too_large(limit_name));
    }
    Ok(n)
}

// spec: RND#helpers
fn take_alphabet(alphabet: Dynamic) -> Result<Vec<char>, Box<EvalAltResult>> {
    let alphabet: Vec<char> = take_string("alphabet", alphabet)?.chars().collect();
    if alphabet.len() < 2 {
        return Err(format!(
            "`alphabet` must hold at least two characters, got {}",
            alphabet.len()
        )
        .into());
    }
    let mut seen = HashSet::with_capacity(alphabet.len());
    if let Some(repeat) = alphabet.iter().find(|c| !seen.insert(**c)) {
        return Err(format!("`alphabet` holds {repeat:?} more than once").into());
    }
    Ok(alphabet)
}

/// The most UTF-8 bytes any character of `alphabet` takes.
fn widest(alphabet: &[char]) -> usize {
    alphabet.iter().map(|c| c.len_utf8()).max().unwrap_or(1)
}

// spec: RND#byte-stream
fn byte_stream(seed: &Seed, fields: &[Field<'_>]) -> Result<ChaCha20, Box<EvalAltResult>> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(seed.expose())
        .expect("HMAC takes a key of any length");
    mac.update(&encode(fields)?);
    let key: [u8; 32] = mac.finalize().into_bytes().into();
    Ok(ChaCha20::new(&key.into(), &[0u8; 12].into()))
}

fn take_bytes(stream: &mut ChaCha20, buf: &mut [u8]) {
    buf.fill(0);
    stream.apply_keystream(buf);
}

fn allocation_failed() -> Box<EvalAltResult> {
    "not enough memory for the requested value".into()
}

// spec: RND#values
fn random_bytes(seed: &Seed, label: &str, n: usize) -> Result<Vec<u8>, Box<EvalAltResult>> {
    let mut stream = byte_stream(
        seed,
        &[
            Field::Str(RANDOM_BYTES),
            Field::Str(label),
            Field::Int(n as u64),
        ],
    )?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(n)
        .map_err(|_| allocation_failed())?;
    bytes.resize(n, 0);
    take_bytes(&mut stream, &mut bytes);
    Ok(bytes)
}

// spec: RND#values
fn random_string(
    seed: &Seed,
    label: &str,
    length: usize,
    alphabet: &[char],
) -> Result<String, Box<EvalAltResult>> {
    let alphabet_text: String = alphabet.iter().collect();
    let mut stream = byte_stream(
        seed,
        &[
            Field::Str(RANDOM_STRING),
            Field::Str(label),
            Field::Int(length as u64),
            Field::Str(&alphabet_text),
        ],
    )?;
    let mut out = String::new();
    out.try_reserve(length.saturating_mul(widest(alphabet)))
        .map_err(|_| allocation_failed())?;
    let mut produced = 0;
    let mut word = [0u8; 4];
    while produced < length {
        take_bytes(&mut stream, &mut word);
        if let Some(index) = pick(u32::from_le_bytes(word), alphabet.len()) {
            out.push(alphabet[index]);
            produced += 1;
        }
    }
    Ok(out)
}

// spec: RND#values
/// The position in an alphabet of `k` characters that the word `w` picks, or
/// `None` where `w` falls in the top `2^32 mod k` words, which would favour the
/// first characters were they kept.
fn pick(w: u32, k: usize) -> Option<usize> {
    let k = k as u64;
    let span = 1u64 << 32;
    let w = u64::from(w);
    (w < span - span % k).then(|| (w % k) as usize)
}

// spec: RND#values
fn uuid_v4(seed: &Seed, label: &str) -> Result<String, Box<EvalAltResult>> {
    let mut stream = byte_stream(seed, &[Field::Str(UUID_V4), Field::Str(label)])?;
    let mut bytes = [0u8; 16];
    take_bytes(&mut stream, &mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(uuid::Uuid::from_bytes(bytes).hyphenated().to_string())
}

#[cfg(test)]
mod tests;
