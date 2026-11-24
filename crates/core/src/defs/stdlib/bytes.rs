//! The `bytes` arguments of the `std` helpers.

use bytes::Bytes;
use rhai::{Dynamic, EvalAltResult};

use super::fields::{Field, encode};
use crate::defs::file::{Directory, File};

/// The bytes a script passed for a `bytes` argument.
pub(super) struct ByteArg {
    pub(super) bytes: Bytes,
    /// Whether they came from the definition bundle.
    bundle: bool,
}

impl ByteArg {
    /// A string argument, read as its UTF-8 encoding.
    pub(super) fn text(text: String) -> Self {
        Self {
            bytes: text.into(),
            bundle: false,
        }
    }

    // spec: SEC#redaction
    /// The bytes to look for a withheld value in: none for a bundle file or
    /// directory, which hold only what the definition holds.
    pub(super) fn withholdable(&self) -> Option<&[u8]> {
        (!self.bundle).then_some(&self.bytes[..])
    }
}

// spec: ENC#byte-arguments
/// Convert a string, Blob, `File` or `Directory` to its bytes, or throw
/// naming `what` and the actual type.
pub(super) fn take_byte_arg(what: &str, value: Dynamic) -> Result<ByteArg, Box<EvalAltResult>> {
    let type_name = value.type_name();
    // take: dispatch
    if value.is_string() {
        let text = value.into_immutable_string()?;
        return Ok(ByteArg::text(text.into_owned()));
    }
    // take: dispatch
    if value.is_blob() {
        return Ok(ByteArg {
            bytes: value.into_blob()?.into(),
            bundle: false,
        });
    }
    // take: dispatch
    if let Some(file) = value.clone().try_cast::<File>() {
        return Ok(ByteArg {
            bytes: file.contents,
            bundle: true,
        });
    }
    // take: dispatch
    if let Some(dir) = value.try_cast::<Directory>() {
        return Ok(ByteArg {
            bytes: directory_encoding(&dir)?.into(),
            bundle: true,
        });
    }
    Err(format!("`{what}` must be a string, Blob, File or Directory, got {type_name}").into())
}

// spec: ENC#directory-encoding
fn directory_encoding(dir: &Directory) -> Result<Vec<u8>, Box<EvalAltResult>> {
    let mut files: Vec<_> = dir.files.iter().collect();
    files.sort_by(|(a, _), (b, _)| a.as_bytes().cmp(b.as_bytes()));
    let mut out = Vec::new();
    for (path, contents) in files {
        out.extend(encode(&[Field::Str(path), Field::Bytes(contents)])?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
