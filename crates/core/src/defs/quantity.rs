//! Quantities: a number with an optional decimal or binary suffix, read as a
//! whole amount, for the `quantity` kind and container memory limits.

use num_bigint::BigInt;
use winnow::{
    Parser,
    ascii::digit1,
    combinator::{alt, eof, opt, preceded},
    token::{one_of, rest},
};

/// The decimal suffixes and the power of ten each stands for.
const DECIMAL: [(&str, u32); 6] = [
    ("k", 3),
    ("M", 6),
    ("G", 9),
    ("T", 12),
    ("P", 15),
    ("E", 18),
];

/// The binary suffixes and the power of two each stands for.
const BINARY: [(&str, u32); 6] = [
    ("Ki", 10),
    ("Mi", 20),
    ("Gi", 30),
    ("Ti", 40),
    ("Pi", 50),
    ("Ei", 60),
];

/// The parts of a quantity's text.
struct Parts<'a> {
    negative: bool,
    whole: &'a str,
    fraction: &'a str,
    suffix: Suffix<'a>,
}

enum Suffix<'a> {
    Decimal(u32),
    Binary(u32),
    Exponent { negative: bool, digits: &'a str },
}

/// Read `text` as a quantity, and give its amount.
// spec: ACN#quantities
pub fn parse_quantity(text: &str) -> Result<i64, String> {
    let parts = match parts.parse(text) {
        Ok(parts) => parts,
        Err(_) => return Err(not_a_quantity(text)),
    };
    amount(&parts)
}

/// Write `n` as its shortest quantity: with whichever suffix it is an exact
/// multiple of to two decimal places gives the shortest text, or as a plain
/// integer where none is shorter. A tie goes to a binary suffix over a decimal
/// one, and to a larger suffix over a smaller one.
// spec: ACN#quantities
pub fn format_quantity(n: i64) -> String {
    let magnitude = i128::from(n).abs();
    let sign = if n < 0 { "-" } else { "" };
    let binary = BINARY
        .iter()
        .rev()
        .map(|&(suffix, power)| (suffix, 1i128 << power));
    let decimal = DECIMAL
        .iter()
        .rev()
        .map(|&(suffix, power)| (suffix, 10i128.pow(power)));
    let plain = n.to_string();
    binary
        .chain(decimal)
        .filter(|&(_, unit)| magnitude >= unit && (magnitude * 100) % unit == 0)
        .map(|(suffix, unit)| {
            let hundredths = magnitude * 100 / unit;
            let (whole, fraction) = (hundredths / 100, hundredths % 100);
            match fraction {
                0 => format!("{sign}{whole}{suffix}"),
                f if f % 10 == 0 => format!("{sign}{whole}.{}{suffix}", f / 10),
                f => format!("{sign}{whole}.{f:02}{suffix}"),
            }
        })
        .min_by_key(String::len)
        .filter(|text| text.len() <= plain.len())
        .unwrap_or(plain)
}

fn parts<'a>(input: &mut &'a str) -> winnow::Result<Parts<'a>> {
    let negative = sign.parse_next(input)?;
    let (whole, fraction) = number.parse_next(input)?;
    let suffix = suffix.parse_next(input)?;
    eof.parse_next(input)?;
    Ok(Parts {
        negative,
        whole,
        fraction,
        suffix,
    })
}

fn sign(input: &mut &str) -> winnow::Result<bool> {
    opt(one_of(['+', '-']))
        .map(|sign| sign == Some('-'))
        .parse_next(input)
}

/// One or more digits with at most one `.` before, between or after them, as
/// the digits before the `.` and those after it.
fn number<'a>(input: &mut &'a str) -> winnow::Result<(&'a str, &'a str)> {
    alt((
        (digit1, opt(preceded('.', opt(digit1)))).map(
            |(whole, fraction): (&str, Option<Option<&str>>)| {
                (whole, fraction.flatten().unwrap_or(""))
            },
        ),
        preceded('.', digit1).map(|fraction| ("", fraction)),
    ))
    .parse_next(input)
}

fn suffix<'a>(input: &mut &'a str) -> winnow::Result<Suffix<'a>> {
    // An exponent goes first, so that `E` reads as exa only with no digits
    // after it.
    alt((
        preceded(one_of(['e', 'E']), (sign, digit1))
            .map(|(negative, digits)| Suffix::Exponent { negative, digits }),
        rest.verify_map(|suffix: &str| match suffix {
            "" => Some(Suffix::Decimal(0)),
            _ => BINARY
                .iter()
                .find(|(s, _)| *s == suffix)
                .map(|&(_, power)| Suffix::Binary(power))
                .or_else(|| {
                    DECIMAL
                        .iter()
                        .find(|(s, _)| *s == suffix)
                        .map(|&(_, power)| Suffix::Decimal(power))
                }),
        }),
    ))
    .parse_next(input)
}

/// The amount of a quantity, computed exactly.
// spec: ACN#quantities
fn amount(parts: &Parts<'_>) -> Result<i64, String> {
    const NOT_WHOLE: &str = "must come to a whole number";
    const OUT_OF_RANGE: &str = "must be within the signed 64-bit range";

    let digits = format!("{}{}", parts.whole, parts.fraction);
    let significant = digits.trim_start_matches('0').trim_end_matches('0');
    if significant.is_empty() {
        return Ok(0);
    }
    // The amount is mantissa × 10^ten × 2^two, where the mantissa's trailing
    // zeros have been moved into the power of ten.
    let trailing_zeros = digits.trim_end_matches('0').len().abs_diff(digits.len());
    let mut ten = i128::try_from(trailing_zeros).expect("a string's length fits i128")
        - i128::try_from(parts.fraction.len()).expect("a string's length fits i128");
    let mut two = 0;
    match parts.suffix {
        Suffix::Decimal(power) => ten += i128::from(power),
        Suffix::Binary(power) => two = power,
        Suffix::Exponent { negative, digits } => {
            let Ok(exponent) = digits.parse::<i64>() else {
                return Err(if negative { NOT_WHOLE } else { OUT_OF_RANGE }.to_owned());
            };
            ten += if negative {
                -i128::from(exponent)
            } else {
                i128::from(exponent)
            };
        }
    }

    // A non-zero mantissa is at least 1, so a power of ten from 19 up puts
    // the amount past the largest signed 64-bit integer.
    if ten >= 19 {
        return Err(OUT_OF_RANGE.to_owned());
    }
    // A mantissa with no trailing zero divides by 10^n only if it divides by
    // 5^n, which needs more than n × log10(5) of its digits.
    if ten < 0 && ten.unsigned_abs() > 2 * significant.len() as u128 + 2 {
        return Err(NOT_WHOLE.to_owned());
    }

    let mantissa: BigInt = significant.parse().expect("digits parse as an integer");
    let scaled = mantissa << two;
    let amount = if ten >= 0 {
        scaled * BigInt::from(10).pow(u32::try_from(ten).expect("ten is under 19"))
    } else {
        let divisor =
            BigInt::from(10).pow(u32::try_from(-ten).expect("ten is bounded by the input"));
        if &scaled % &divisor != BigInt::ZERO {
            return Err(NOT_WHOLE.to_owned());
        }
        scaled / divisor
    };
    let amount = if parts.negative { -amount } else { amount };
    i64::try_from(&amount).map_err(|_| OUT_OF_RANGE.to_owned())
}

/// The reason `text` is not a quantity, with the quantity it most likely
/// meant where only its suffix is wrong.
// spec: ACN#quantities
fn not_a_quantity(text: &str) -> String {
    const PLAIN: &str = "must be a quantity, such as 512Mi or 1.5G";
    let written_number = (sign, number).take();
    let Ok((written, unit)) = (written_number, rest).parse(text) else {
        return PLAIN.to_owned();
    };
    let Some(suffix) = likely_suffix(unit) else {
        return PLAIN.to_owned();
    };
    let milli = if unit == "m" { "m is milli; " } else { "" };
    format!("{milli}did you mean {written}{suffix}?")
}

/// The suffix most likely meant by `unit`: a single letter is binary, as in
/// the convention where `m` means mebibytes, as is a letter followed by `i`,
/// and a letter followed by `B` is decimal.
fn likely_suffix(unit: &str) -> Option<&'static str> {
    let lower = unit.to_ascii_lowercase();
    let (letter, binary, letters): (u8, bool, &[u8]) = match lower.as_bytes() {
        [b'b'] => return Some(""),
        [letter] => (*letter, true, b"kmgtp"),
        [letter, b'i'] | [letter, b'i', b'b'] => (*letter, true, b"kmgtpe"),
        [letter, b'b'] => (*letter, false, b"kmgtpe"),
        _ => return None,
    };
    let index = letters.iter().position(|&l| l == letter)?;
    let suffix = if binary {
        BINARY[index].0
    } else {
        DECIMAL[index].0
    };
    (suffix != unit).then_some(suffix)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    /// The cases the console's port of these functions is tested against too,
    /// so the two cannot drift apart.
    fn vectors() -> Value {
        serde_json::from_str(include_str!("quantity/vectors.json")).expect("vectors are JSON")
    }

    fn amount(value: &Value) -> i64 {
        value
            .as_str()
            .expect("an amount is a string")
            .parse()
            .expect("an amount is an i64")
    }

    #[test]
    fn parsing_matches_the_shared_vectors() {
        for group in vectors()["parse"].as_array().unwrap() {
            for case in group["cases"].as_array().unwrap() {
                let text = case["text"].as_str().unwrap();
                let expected = match case.get("amount") {
                    Some(n) => Ok(amount(n)),
                    None => Err(case["error"].as_str().unwrap().to_owned()),
                };
                assert_eq!(
                    parse_quantity(text),
                    expected,
                    "{}: {text:?}",
                    group["case"]
                );
            }
        }
    }

    #[test]
    fn formatting_matches_the_shared_vectors() {
        for case in vectors()["format"].as_array().unwrap() {
            let n = amount(&case["amount"]);
            assert_eq!(format_quantity(n), case["text"].as_str().unwrap(), "{n}");
        }
    }

    #[test]
    fn a_formatted_quantity_parses_back() {
        for case in vectors()["format"].as_array().unwrap() {
            let n = amount(&case["amount"]);
            assert_eq!(parse_quantity(&format_quantity(n)), Ok(n), "{n}");
        }
    }
}
