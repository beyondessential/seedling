//! The helpers of the `std` module.

use base64::Engine as _;
use rhai::{Dynamic, EvalAltResult, FuncRegistration, NativeCallContext, Position};

use self::bytes::ByteArg;
use crate::runtime::{barrier::runtime::is_in_action_closure, seeds};

mod bytes;
mod encoding;
mod fields;
pub(crate) mod kinds;
mod random;

// spec: STD#the-std-module
/// The `std` module, holding every helper BSL offers.
pub fn module() -> rhai::Module {
    let mut module = rhai::Module::new();
    kinds::register(&mut module);
    random::register(&mut module);
    encoding::register(&mut module);
    module
}

/// The limits the `std` helpers hold an evaluation to, which the engine has
/// none of its own for, so each evaluation of an engine starts with them as
/// its tag.
// spec: LIM#limits
#[derive(Clone, Copy, Debug)]
pub struct HelperLimits {
    pub derived_values: usize,
    pub password_hashes: usize,
}

/// The limits the calling evaluation is held to.
///
/// The tag is the runtime's, never the script's: an engine that does not
/// carry the limits was not built by `setup_language`, and its helpers derive
/// nothing rather than deriving without them.
fn helper_limits(ctx: &NativeCallContext) -> Result<HelperLimits, Box<EvalAltResult>> {
    ctx.tag()
        .and_then(|tag| tag.read_lock::<HelperLimits>().map(|l| *l))
        .ok_or_else(|| {
            "this evaluation carries no limits for its helpers, so it derives nothing".into()
        })
}

/// A value beyond the engine's limit for it, which a script cannot catch.
fn too_large(what: &str) -> Box<EvalAltResult> {
    EvalAltResult::ErrorDataTooLarge(what.into(), Position::NONE).into()
}

// spec: LIM#limits
/// Refuse a result of `len` beyond `limit`, 0 being no limit.
fn within(len: usize, limit: usize, what: &str) -> Result<(), Box<EvalAltResult>> {
    if limit != 0 && len > limit {
        return Err(too_large(what));
    }
    Ok(())
}

/// What the engine calls a string in a size error.
const STRING: &str = "Length of string";
/// What the engine calls a Blob in a size error.
const BLOB: &str = "Size of BLOB";

// spec: SEC#redaction
/// The forms of a Blob that the runtime withholds: its bytes, its UTF-8
/// reading, and each text encoding BSL offers of it, including its string
/// form, which runs its hex digits together in groups of eight bytes.
fn blob_forms(bytes: &[u8]) -> Vec<Vec<u8>> {
    let rendered = Dynamic::from_blob(bytes.to_vec()).to_string();
    let digits = rendered.trim_start_matches('[').trim_end_matches(']');
    vec![
        bytes.to_vec(),
        String::from_utf8_lossy(bytes).into_owned().into_bytes(),
        digits.as_bytes().to_vec(),
        hex::encode(bytes).into_bytes(),
        base64::engine::general_purpose::STANDARD
            .encode(bytes)
            .into_bytes(),
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(bytes)
            .into_bytes(),
    ]
}

type HelperResult = Result<Dynamic, Box<EvalAltResult>>;

fn unary(
    module: &mut rhai::Module,
    name: &str,
    f: fn(&NativeCallContext, Dynamic) -> HelperResult,
) {
    FuncRegistration::new(name)
        .set_into_module(module, move |ctx: NativeCallContext, a: Dynamic| f(&ctx, a));
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
// spec: QUOT#results
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
    let held: Vec<&[u8]> = args.iter().filter_map(|a| a.withholdable()).collect();
    if is_in_action_closure() {
        // spec: SEC#matching
        let sensitive = seeds::operation_scope().map(|scope| scope.values());
        if !sensitive.is_some_and(|s| held.iter().any(|a| s.found_in_bytes(a))) {
            return Ok(());
        }
        // spec: LIM#limits
        if !seeds::count_derivation(helper_limits(ctx)?.derived_values) {
            return Err(too_large("Number of derived values"));
        }
        seeds::record_operation_derived(forms());
        return Ok(());
    }
    let Some(dependence) = seeds::dependence(&held) else {
        return Ok(());
    };
    // spec: LIM#limits
    if !seeds::withhold(dependence, forms(), helper_limits(ctx)?.derived_values) {
        return Err(too_large("Number of derived values"));
    }
    Ok(())
}
