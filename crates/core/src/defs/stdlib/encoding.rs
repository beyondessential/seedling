//! Digests and encodings: helpers that turn bytes into text and back, digest
//! them, authenticate them under a key, and name them with a UUID.

use base64::{Engine as _, engine::general_purpose};
use hmac::{Hmac, KeyInit, Mac};
use rhai::{Dynamic, EvalAltResult, FuncRegistration, NativeCallContext};
use sha2::{Digest, Sha256, Sha512};

use super::{
    BLOB, STRING, blob_forms,
    bytes::{ByteArg, take_byte_arg},
    helper_limits, too_large, within,
};
use crate::{
    defs::take::take_string,
    runtime::{barrier::runtime::is_in_action_closure, seeds},
};

type HelperResult = Result<Dynamic, Box<EvalAltResult>>;

pub(super) fn register(module: &mut rhai::Module) {
    // spec: ENC#encodings
    unary(module, "encode_hex", |ctx, data| {
        let data = take_byte_arg("data", data)?;
        within(data.bytes.len().saturating_mul(2), max_string(ctx), STRING)?;
        text_result(ctx, &[&data], hex::encode(&data.bytes))
    });
    // spec: ENC#encodings
    unary(module, "decode_hex", |ctx, text| {
        let text = take_text(text)?;
        let bytes = hex::decode(&text.bytes).map_err(|e| not_encoded("base16", hex_error(e)))?;
        blob_result(ctx, &[&text], bytes)
    });
    // spec: ENC#encodings
    unary(module, "encode_base64", |ctx, data| {
        let data = take_byte_arg("data", data)?;
        within(
            data.bytes.len().div_ceil(3).saturating_mul(4),
            max_string(ctx),
            STRING,
        )?;
        text_result(ctx, &[&data], general_purpose::STANDARD.encode(&data.bytes))
    });
    // spec: ENC#encodings
    unary(module, "decode_base64", |ctx, text| {
        let text = take_text(text)?;
        let bytes = general_purpose::STANDARD
            .decode(&text.bytes)
            .map_err(|e| not_encoded("base64", base64_error(e)))?;
        blob_result(ctx, &[&text], bytes)
    });
    // spec: ENC#encodings
    unary(module, "encode_base64url", |ctx, data| {
        let data = take_byte_arg("data", data)?;
        within(
            data.bytes.len().saturating_mul(4).div_ceil(3),
            max_string(ctx),
            STRING,
        )?;
        text_result(
            ctx,
            &[&data],
            general_purpose::URL_SAFE_NO_PAD.encode(&data.bytes),
        )
    });
    // spec: ENC#encodings
    unary(module, "decode_base64url", |ctx, text| {
        let text = take_text(text)?;
        let bytes = general_purpose::URL_SAFE_NO_PAD
            .decode(&text.bytes)
            .map_err(|e| not_encoded("base64url", base64_error(e)))?;
        blob_result(ctx, &[&text], bytes)
    });
    // spec: ENC#encodings
    unary(module, "encode_utf8", |ctx, text| {
        let text = take_text(text)?;
        within(text.bytes.len(), max_blob(ctx), BLOB)?;
        let bytes = text.bytes.to_vec();
        blob_result(ctx, &[&text], bytes)
    });
    // spec: ENC#encodings
    unary(module, "decode_utf8", |ctx, data| {
        let data = take_byte_arg("data", data)?;
        within(data.bytes.len(), max_string(ctx), STRING)?;
        let text = String::from_utf8(data.bytes.to_vec())
            .map_err(|e| -> Box<EvalAltResult> { format!("`data` is not UTF-8: {e}").into() })?;
        text_result(ctx, &[&data], text)
    });

    // spec: ENC#digests
    unary(module, "digest_sha256", |ctx, data| {
        let data = take_byte_arg("data", data)?;
        blob_result(ctx, &[&data], Sha256::digest(&data.bytes).to_vec())
    });
    // spec: ENC#digests
    unary(module, "digest_sha512", |ctx, data| {
        let data = take_byte_arg("data", data)?;
        blob_result(ctx, &[&data], Sha512::digest(&data.bytes).to_vec())
    });
    // spec: ENC#digests
    unary(module, "digest_blake2b", |ctx, data| {
        let data = take_byte_arg("data", data)?;
        blob_result(
            ctx,
            &[&data],
            blake2::Blake2b512::digest(&data.bytes).to_vec(),
        )
    });
    // spec: ENC#digests
    unary(module, "digest_blake3", |ctx, data| {
        let data = take_byte_arg("data", data)?;
        blob_result(ctx, &[&data], blake3::hash(&data.bytes).as_bytes().to_vec())
    });

    // spec: ENC#message-authentication
    binary(module, "hmac_sha256", |ctx, key, message| {
        let (key, message) = (
            take_byte_arg("key", key)?,
            take_byte_arg("message", message)?,
        );
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&key.bytes)
            .expect("HMAC takes a key of any length");
        mac.update(&message.bytes);
        blob_result(ctx, &[&key, &message], mac.finalize().into_bytes().to_vec())
    });
    // spec: ENC#message-authentication
    binary(module, "hmac_sha512", |ctx, key, message| {
        let (key, message) = (
            take_byte_arg("key", key)?,
            take_byte_arg("message", message)?,
        );
        let mut mac = <Hmac<Sha512> as KeyInit>::new_from_slice(&key.bytes)
            .expect("HMAC takes a key of any length");
        mac.update(&message.bytes);
        blob_result(ctx, &[&key, &message], mac.finalize().into_bytes().to_vec())
    });

    // spec: ENC#name-based-uuids
    binary(module, "uuid_v5", |ctx, namespace, name| {
        let namespace = take_string("namespace", namespace)?;
        let name = take_byte_arg("name", name)?;
        let uuid = uuid::Uuid::new_v5(&parse_hyphenated(&namespace)?, &name.bytes);
        let namespace = ByteArg::text(namespace);
        text_result(ctx, &[&namespace, &name], uuid.hyphenated().to_string())
    });
}

fn unary(
    module: &mut rhai::Module,
    name: &str,
    f: fn(&NativeCallContext, Dynamic) -> HelperResult,
) {
    FuncRegistration::new(name)
        .set_into_module(module, move |ctx: NativeCallContext, a: Dynamic| f(&ctx, a));
}

fn binary(
    module: &mut rhai::Module,
    name: &str,
    f: fn(&NativeCallContext, Dynamic, Dynamic) -> HelperResult,
) {
    FuncRegistration::new(name).set_into_module(
        module,
        move |ctx: NativeCallContext, a: Dynamic, b: Dynamic| f(&ctx, a, b),
    );
}

fn take_text(text: Dynamic) -> Result<ByteArg, Box<EvalAltResult>> {
    Ok(ByteArg::text(take_string("text", text)?))
}

// spec: SEC#redaction
/// A decoding failure, naming where `text` went wrong but never what it
/// holds: it may be a secret parameter's value, and errors are not redacted.
fn not_encoded(encoding: &str, why: String) -> Box<EvalAltResult> {
    format!("`text` is not {encoding}: {why}").into()
}

fn hex_error(e: hex::FromHexError) -> String {
    match e {
        hex::FromHexError::InvalidHexCharacter { index, .. } => {
            format!("invalid digit at position {index}")
        }
        hex::FromHexError::OddLength | hex::FromHexError::InvalidStringLength => {
            "odd number of digits".into()
        }
    }
}

fn base64_error(e: base64::DecodeError) -> String {
    match e {
        base64::DecodeError::InvalidByte(offset, _) => {
            format!("invalid character at position {offset}")
        }
        base64::DecodeError::InvalidLength(_) => "invalid length".into(),
        base64::DecodeError::InvalidLastSymbol { offset, .. } => {
            format!("non-zero trailing bits at position {offset}")
        }
        base64::DecodeError::InvalidPadding => "invalid padding".into(),
    }
}

fn max_string(ctx: &NativeCallContext) -> usize {
    ctx.engine().max_string_size()
}

// spec: LIM#limits
/// The engine counts a Blob's bytes against its array limit.
fn max_blob(ctx: &NativeCallContext) -> usize {
    ctx.engine().max_array_size()
}

// spec: ENC#results
fn text_result(ctx: &NativeCallContext, args: &[&ByteArg], text: String) -> HelperResult {
    within(text.len(), max_string(ctx), STRING)?;
    settle(ctx, args, || vec![text.as_bytes().to_vec()])?;
    Ok(text.into())
}

// spec: ENC#results
fn blob_result(ctx: &NativeCallContext, args: &[&ByteArg], bytes: Vec<u8>) -> HelperResult {
    within(bytes.len(), max_blob(ctx), BLOB)?;
    settle(ctx, args, || blob_forms(&bytes))?;
    Ok(Dynamic::from_blob(bytes))
}

// spec: SEC#redaction
/// Withhold a result when any of its arguments contains a withheld value.
/// Results computed in an action closure are the operation's own.
fn settle(
    ctx: &NativeCallContext,
    args: &[&ByteArg],
    forms: impl FnOnce() -> Vec<Vec<u8>>,
) -> Result<(), Box<EvalAltResult>> {
    if is_in_action_closure() {
        return Ok(());
    }
    let held: Vec<&[u8]> = args.iter().filter_map(|a| a.withholdable()).collect();
    let Some(dependence) = seeds::dependence(&held) else {
        return Ok(());
    };
    // spec: LIM#limits
    if !seeds::withhold(dependence, forms(), helper_limits(ctx)?.derived_values) {
        return Err(too_large("Number of derived values"));
    }
    Ok(())
}

// spec: ENC#name-based-uuids
/// A UUID written in hyphenated form, in either case. The uuid parser also
/// takes the simple, braced and URN forms, so the shape is checked first.
fn parse_hyphenated(text: &str) -> Result<uuid::Uuid, Box<EvalAltResult>> {
    let shaped = text.len() == 36
        && text.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        });
    if !shaped {
        return Err(format!(
            "`namespace` must be a UUID in hyphenated form, as 8-4-4-4-12 hexadecimal digits, got a value of {} characters",
            text.chars().count()
        )
        .into());
    }
    Ok(uuid::Uuid::parse_str(text).expect("a hyphenated UUID parses"))
}

#[cfg(test)]
mod tests;
