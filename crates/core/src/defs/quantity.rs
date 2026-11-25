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

/// Write `n` as a quantity: with the largest binary suffix it is a multiple
/// of to two decimal places, otherwise the largest such decimal suffix,
/// otherwise as a plain integer.
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
    for (suffix, unit) in binary.chain(decimal) {
        if magnitude >= unit && (magnitude * 100) % unit == 0 {
            let hundredths = magnitude * 100 / unit;
            let (whole, fraction) = (hundredths / 100, hundredths % 100);
            return match fraction {
                0 => format!("{sign}{whole}{suffix}"),
                f if f % 10 == 0 => format!("{sign}{whole}.{}{suffix}", f / 10),
                f => format!("{sign}{whole}.{f:02}{suffix}"),
            };
        }
    }
    n.to_string()
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
    use super::*;

    #[test]
    fn every_suffix_multiplies_the_number() {
        for (text, amount) in [
            ("0", 0),
            ("7", 7),
            ("1k", 1_000),
            ("1M", 1_000_000),
            ("1G", 1_000_000_000),
            ("1T", 1_000_000_000_000),
            ("1P", 1_000_000_000_000_000),
            ("1E", 1_000_000_000_000_000_000),
            ("1Ki", 1 << 10),
            ("1Mi", 1 << 20),
            ("1Gi", 1 << 30),
            ("1Ti", 1 << 40),
            ("1Pi", 1 << 50),
            ("1Ei", 1 << 60),
        ] {
            assert_eq!(parse_quantity(text), Ok(amount), "{text}");
        }
    }

    #[test]
    fn an_exponent_scales_by_a_power_of_ten() {
        for (text, amount) in [
            ("1e3", 1_000),
            ("1E3", 1_000),
            ("1e+3", 1_000),
            ("15000e-3", 15),
            ("2.5e2", 250),
            ("0e99999999999999999999", 0),
        ] {
            assert_eq!(parse_quantity(text), Ok(amount), "{text}");
        }
    }

    #[test]
    fn e_alone_is_exa_and_with_digits_an_exponent() {
        assert_eq!(parse_quantity("2E"), Ok(2_000_000_000_000_000_000));
        assert_eq!(parse_quantity("2E2"), Ok(200));
    }

    #[test]
    fn a_number_may_put_its_point_anywhere() {
        assert_eq!(parse_quantity(".5Gi"), Ok(1 << 29));
        assert_eq!(parse_quantity("5.Gi"), Ok(5 << 30));
        assert_eq!(parse_quantity("1.5Gi"), Ok(1_610_612_736));
        assert_eq!(parse_quantity("1.5k"), Ok(1_500));
        assert_eq!(parse_quantity("007"), Ok(7));
    }

    #[test]
    fn a_sign_is_taken() {
        assert_eq!(parse_quantity("-512Mi"), Ok(-(512 << 20)));
        assert_eq!(parse_quantity("+1k"), Ok(1_000));
        assert_eq!(parse_quantity("-0"), Ok(0));
    }

    #[test]
    fn the_signed_64_bit_range_bounds_the_amount() {
        assert_eq!(parse_quantity("9223372036854775807"), Ok(i64::MAX));
        assert_eq!(parse_quantity("-9223372036854775808"), Ok(i64::MIN));
        assert_eq!(parse_quantity("-8Ei"), Ok(i64::MIN));
        for text in [
            "9223372036854775808",
            "8Ei",
            "10E",
            "1e19",
            "1e99999999999999999999",
        ] {
            assert_eq!(
                parse_quantity(text),
                Err("must be within the signed 64-bit range".to_owned()),
                "{text}"
            );
        }
    }

    #[test]
    fn a_fractional_amount_is_rejected() {
        for text in ["1.1Ki", "1e-3", "1500e-3", "0.5", "1e-99999999999999999999"] {
            assert_eq!(
                parse_quantity(text),
                Err("must come to a whole number".to_owned()),
                "{text}"
            );
        }
        assert_eq!(parse_quantity("0.5Ki"), Ok(512));
        assert_eq!(
            parse_quantity("0.000000000000000000867361737988403547205962240695953369140625Ei"),
            Ok(1)
        );
    }

    #[test]
    fn anything_else_is_not_a_quantity() {
        for text in [
            "", " 1", "1 ", "1 Ki", ".", "+", "1..2", "1.2.3", "1e", "1e+", "1e3Ki", "1kk", "1u",
            "1n", "0x10", "1,000", "--1",
        ] {
            assert_eq!(
                parse_quantity(text),
                Err("must be a quantity, such as 512Mi or 1.5G".to_owned()),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_wrong_suffix_suggests_the_one_likely_meant() {
        for (text, error) in [
            ("512m", "m is milli; did you mean 512Mi?"),
            ("512mi", "did you mean 512Mi?"),
            ("1ki", "did you mean 1Ki?"),
            ("1K", "did you mean 1Ki?"),
            ("512MiB", "did you mean 512Mi?"),
            ("1MB", "did you mean 1M?"),
            ("1KB", "did you mean 1k?"),
            ("1g", "did you mean 1Gi?"),
            ("-1.5g", "did you mean -1.5Gi?"),
            ("64B", "did you mean 64?"),
        ] {
            assert_eq!(parse_quantity(text), Err(error.to_owned()), "{text}");
        }
    }

    #[test]
    fn formatting_picks_the_largest_exact_suffix() {
        for (n, text) in [
            (0, "0"),
            (7, "7"),
            (1_000, "1k"),
            (1_500, "1.5k"),
            (1 << 20, "1Mi"),
            (64 << 20, "64Mi"),
            (15872 << 20, "15.5Gi"),
            (1_610_612_736, "1.5Gi"),
            (-(512 << 20), "-512Mi"),
            (1_230, "1.23k"),
            (1_234, "1234"),
            (1_001, "1001"),
            (i64::MIN, "-8Ei"),
            (i64::MAX, "9223372036854775807"),
        ] {
            assert_eq!(format_quantity(n), text, "{n}");
        }
    }

    #[test]
    fn a_formatted_quantity_parses_back() {
        for n in [
            0,
            1,
            1_000,
            1_500,
            64 << 20,
            15872 << 20,
            -(1 << 40),
            i64::MIN,
            i64::MAX,
        ] {
            assert_eq!(parse_quantity(&format_quantity(n)), Ok(n), "{n}");
        }
    }
}
