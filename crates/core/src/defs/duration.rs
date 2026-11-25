//! The values the `duration` kind's parser returns, and the bounds the kind
//! takes.

use std::fmt;

use jiff::{SpanRelativeTo, fmt::friendly::SpanParser};
use rhai::{CustomType, EvalAltResult, FLOAT, INT, TypeBuilder};

use super::{compare::refuse_mixed_comparisons, param_def::line_breaking_character};

const NANOS_PER_MICRO: u64 = 1_000;
const NANOS_PER_MILLI: u64 = 1_000_000;
const NANOS_PER_SECOND: u64 = 1_000_000_000;
const NANOS_PER_MINUTE: u64 = 60 * NANOS_PER_SECOND;
const NANOS_PER_HOUR: u64 = 60 * NANOS_PER_MINUTE;
const NANOS_PER_DAY: u64 = 24 * NANOS_PER_HOUR;

/// A length of time, held as a whole number of nanoseconds.
// spec: DUR#duration
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Duration {
    nanos: u64,
}

/// The designators of the years and months units, which have no fixed length.
// spec: DUR#duration-syntax
const CALENDAR_DESIGNATORS: [&str; 9] = [
    "years", "year", "yrs", "yr", "y", "months", "month", "mos", "mo",
];

impl Duration {
    const NAME: &str = "Duration";

    /// The longest duration.
    // spec: DUR#duration
    pub const MAX: Self = Self {
        nanos: i64::MAX as u64,
    };

    /// The most whole seconds a duration holds.
    pub const MAX_SECS: u64 = Self::MAX.nanos / NANOS_PER_SECOND;

    pub const fn from_secs(secs: u64) -> Self {
        Self {
            nanos: secs * NANOS_PER_SECOND,
        }
    }

    pub const fn from_millis(millis: u64) -> Self {
        Self {
            nanos: millis * NANOS_PER_MILLI,
        }
    }

    /// The duration of `nanos` nanoseconds, or `None` past the longest.
    fn from_nanos(nanos: u64) -> Option<Self> {
        (nanos <= Self::MAX.nanos).then_some(Self { nanos })
    }

    // spec: DUR#duration-syntax
    pub fn parse(text: &str) -> Result<Self, String> {
        if let Some(c) = line_breaking_character(text) {
            return Err(format!(
                "contains U+{:04X}, which a single-line value cannot hold",
                u32::from(c)
            ));
        }
        if text.starts_with(['+', '-']) {
            return Err(format!(
                "{text:?} is not a duration: a duration takes no sign"
            ));
        }
        for word in text.split(|c: char| !c.is_alphabetic()) {
            if word == "ago" {
                return Err(format!(
                    "{text:?} is not a duration: a duration takes no `ago`"
                ));
            }
            if CALENDAR_DESIGNATORS.contains(&word) {
                return Err(format!(
                    "{text:?} is not a duration: years and months have no fixed length, \
                     so write the duration in days or weeks, such as 30d"
                ));
            }
        }
        let span = SpanParser::new()
            .parse_span(text)
            .map_err(|e| format!("{text:?} is not a duration: {e}"))?;
        let duration = span
            .to_duration(SpanRelativeTo::days_are_24_hours())
            .map_err(|e| format!("{text:?} is not a duration: {e}"))?;
        u64::try_from(duration.as_nanos())
            .ok()
            .and_then(Self::from_nanos)
            .ok_or_else(|| {
                format!(
                    "{text:?} is longer than the longest duration, {}",
                    Self::MAX
                )
            })
    }

    /// Whether the duration is a whole number of `unit`.
    pub fn is_whole(self, unit: Precision) -> bool {
        self.nanos.is_multiple_of(unit.nanos())
    }

    /// The duration as a whole number of seconds, or `None` when it is not one.
    pub fn whole_secs(self) -> Option<u64> {
        self.is_whole(Precision::Seconds)
            .then_some(self.nanos / NANOS_PER_SECOND)
    }

    pub fn as_secs_f64(self) -> f64 {
        self.nanos as f64 / NANOS_PER_SECOND as f64
    }

    pub fn is_zero(self) -> bool {
        self.nanos == 0
    }

    /// The duration as a whole number of `unit`, as an integer, or an error
    /// naming `method` when it is not one.
    // spec: DUR#reading-a-duration
    fn whole(self, unit: Precision, method: &str) -> Result<INT, Box<EvalAltResult>> {
        if !self.is_whole(unit) {
            return Err(format!(
                "`{method}`: {self} is not a whole number of {}",
                unit.plural()
            )
            .into());
        }
        Ok(INT::try_from(self.nanos / unit.nanos()).expect("a duration fits an integer"))
    }

    // spec: DUR#arithmetic-on-durations
    fn checked(nanos: Option<u64>, what: &str) -> Result<Self, Box<EvalAltResult>> {
        nanos.and_then(Self::from_nanos).ok_or_else(|| {
            format!("{what} is longer than the longest duration, {}", Self::MAX).into()
        })
    }

    // spec: DUR#arithmetic-on-durations
    fn times(self, n: INT) -> Result<Self, Box<EvalAltResult>> {
        let Ok(n) = u64::try_from(n) else {
            return Err(format!("cannot multiply a duration by a negative number, {n}").into());
        };
        Self::checked(self.nanos.checked_mul(n), &format!("{self} * {n}"))
    }

    // spec: DUR#arithmetic-on-durations
    fn divided(self, n: INT) -> Result<Self, Box<EvalAltResult>> {
        match u64::try_from(n) {
            Ok(0) => Err(format!("cannot divide {self} by zero").into()),
            Ok(n) => Ok(Self {
                nanos: self.nanos / n,
            }),
            Err(_) => Err(format!("cannot divide a duration by a negative number, {n}").into()),
        }
    }
}

// spec: DUR#text-of-a-duration
impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.nanos == 0 {
            return f.write_str("0s");
        }
        let mut rest = self.nanos;
        let mut first = true;
        for unit in Precision::ALL {
            let count = rest / unit.nanos();
            rest %= unit.nanos();
            if count == 0 {
                continue;
            }
            if !first {
                f.write_str(" ")?;
            }
            first = false;
            write!(f, "{count}{}", unit.designator())?;
        }
        Ok(())
    }
}

impl CustomType for Duration {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name(Self::NAME);

        // spec: DUR#reading-a-duration
        builder
            .with_fn("as_seconds", |this: &mut Self| {
                this.whole(Precision::Seconds, "as_seconds")
            })
            .with_fn("as_millis", |this: &mut Self| {
                this.whole(Precision::Millis, "as_millis")
            })
            .with_fn("as_seconds_f", |this: &mut Self| -> FLOAT {
                this.as_secs_f64()
            })
            .with_fn("as_millis_f", |this: &mut Self| -> FLOAT {
                this.nanos as FLOAT / NANOS_PER_MILLI as FLOAT
            });

        // spec: DUR#comparing-durations
        builder
            .with_fn("==", |a: &mut Self, b: Self| *a == b)
            .with_fn("!=", |a: &mut Self, b: Self| *a != b)
            .with_fn("<", |a: &mut Self, b: Self| *a < b)
            .with_fn("<=", |a: &mut Self, b: Self| *a <= b)
            .with_fn(">", |a: &mut Self, b: Self| *a > b)
            .with_fn(">=", |a: &mut Self, b: Self| *a >= b);
        refuse_mixed_comparisons(
            &mut builder,
            Self::NAME,
            "a Duration compares only with a Duration",
        );

        // spec: DUR#arithmetic-on-durations
        builder
            .with_fn(
                "+",
                |a: &mut Self, b: Self| -> Result<Self, Box<EvalAltResult>> {
                    Self::checked(a.nanos.checked_add(b.nanos), &format!("{a} + {b}"))
                },
            )
            .with_fn(
                "-",
                |a: &mut Self, b: Self| -> Result<Self, Box<EvalAltResult>> {
                    a.nanos
                        .checked_sub(b.nanos)
                        .map(|nanos| Self { nanos })
                        .ok_or_else(|| {
                            format!("{a} - {b} is negative, and a duration cannot be").into()
                        })
                },
            )
            .with_fn("*", |a: &mut Self, n: INT| a.times(n))
            .with_fn("*", |n: INT, a: Self| a.times(n))
            .with_fn("/", |a: &mut Self, n: INT| a.divided(n));

        // spec: DUR#text-of-a-duration
        builder
            .on_print(|this: &mut Self| this.to_string())
            .on_debug(|this: &mut Self| this.to_string());
    }
}

/// The unit a `duration` kind's `precision` designates, and the units of a
/// duration's text form, largest first.
// spec: DUR#bounds
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    Days,
    Hours,
    Minutes,
    Seconds,
    Millis,
    Micros,
    Nanos,
}

impl Precision {
    const ALL: [Self; 7] = [
        Self::Days,
        Self::Hours,
        Self::Minutes,
        Self::Seconds,
        Self::Millis,
        Self::Micros,
        Self::Nanos,
    ];

    fn nanos(self) -> u64 {
        match self {
            Self::Days => NANOS_PER_DAY,
            Self::Hours => NANOS_PER_HOUR,
            Self::Minutes => NANOS_PER_MINUTE,
            Self::Seconds => NANOS_PER_SECOND,
            Self::Millis => NANOS_PER_MILLI,
            Self::Micros => NANOS_PER_MICRO,
            Self::Nanos => 1,
        }
    }

    // spec: DUR#text-of-a-duration
    pub fn designator(self) -> &'static str {
        match self {
            Self::Days => "d",
            Self::Hours => "h",
            Self::Minutes => "m",
            Self::Seconds => "s",
            Self::Millis => "ms",
            Self::Micros => "us",
            Self::Nanos => "ns",
        }
    }

    fn plural(self) -> &'static str {
        match self {
            Self::Days => "days",
            Self::Hours => "hours",
            Self::Minutes => "minutes",
            Self::Seconds => "seconds",
            Self::Millis => "milliseconds",
            Self::Micros => "microseconds",
            Self::Nanos => "nanoseconds",
        }
    }

    // spec: DUR#bounds
    pub fn from_designator(designator: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|unit| unit.designator() == designator)
    }
}

/// A `duration` kind's arguments.
// spec: DUR#bounds
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DurationBounds {
    pub min: Option<Duration>,
    pub max: Option<Duration>,
    pub precision: Option<Precision>,
}

impl DurationBounds {
    /// The bounds, or the reason no duration falls within them.
    // spec: DUR#bounds
    pub fn new(
        min: Option<Duration>,
        max: Option<Duration>,
        precision: Option<Precision>,
    ) -> Result<Self, String> {
        let least = min.unwrap_or_default();
        let most = max.unwrap_or(Duration::MAX);
        if least > most {
            return Err(format!("`min` {least} is longer than `max` {most}"));
        }
        if let Some(unit) = precision {
            let first_whole = least.nanos.div_ceil(unit.nanos()) * unit.nanos();
            if first_whole > most.nanos {
                return Err(format!(
                    "no duration from {least} to {most} is a whole number of {}",
                    unit.plural()
                ));
            }
        }
        Ok(Self {
            min,
            max,
            precision,
        })
    }

    /// Check `duration` against the bounds, giving the bound it breaks.
    // spec: DUR#bounds
    pub fn check(&self, duration: Duration) -> Result<(), String> {
        if let Some(min) = self.min
            && duration < min
        {
            return Err(format!("must be at least {min}"));
        }
        if let Some(max) = self.max
            && duration > max
        {
            return Err(format!("must be at most {max}"));
        }
        if let Some(unit) = self.precision
            && !duration.is_whole(unit)
        {
            return Err(format!("must be a whole number of {}", unit.plural()));
        }
        Ok(())
    }
}
