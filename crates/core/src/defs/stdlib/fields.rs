//! The fields that a derivation helper's call encoding, and a directory's
//! encoding, are built from.

use rhai::EvalAltResult;

use super::too_large;

pub(super) enum Field<'a> {
    Str(&'a str),
    Int(u64),
    Bytes(&'a [u8]),
}

// spec: RND#call-encoding
/// The fields' encoding, or an error where a string is too long for its
/// length to be written, as an engine with no string limit allows.
pub(super) fn encode(fields: &[Field<'_>]) -> Result<Vec<u8>, Box<EvalAltResult>> {
    let mut out = Vec::new();
    for field in fields {
        match field {
            Field::Str(s) => {
                let len = u32::try_from(s.len()).map_err(|_| too_large("Length of string"))?;
                out.extend_from_slice(&len.to_be_bytes());
                out.extend_from_slice(s.as_bytes());
            }
            Field::Int(n) => out.extend_from_slice(&n.to_be_bytes()),
            Field::Bytes(b) => {
                out.extend_from_slice(&(b.len() as u64).to_be_bytes());
                out.extend_from_slice(b);
            }
        }
    }
    Ok(out)
}
