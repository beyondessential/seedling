//! The sensitive values the runtime withholds from operators, and how it
//! finds and masks them in what it reports.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashSet},
    fmt,
    ops::ControlFlow,
    sync::{Arc, OnceLock},
};

use parking_lot::Mutex;
use siphasher::sip::SipHasher13;

/// What an occurrence of a sensitive value in free text is replaced by.
// spec: SEC#withheld-values
pub const REDACTED: &str = "<redacted>";

/// A set of sensitive values, held only to find them in other text.
///
/// Each is held by its length, as a keyed hash of its bytes and a keyed hash
/// of its rolling hash: a fixed size however long the value, and nothing a
/// value could be read back from. A hash that collides only withholds text
/// that held no sensitive value. Copies of a set share its storage until one
/// is changed.
// spec: SEC#sensitive-value
#[derive(Clone, Default)]
pub struct SensitiveValues(Arc<BTreeMap<usize, Hashes>>);

#[derive(Clone, Default)]
struct Hashes {
    rolling: HashSet<u64>,
    bytes: HashSet<u64>,
}

/// The modulus of the rolling hash, the Mersenne prime 2^61 - 1.
const ROLLING_MODULUS: u64 = (1 << 61) - 1;

/// The keys the runtime hashes sensitive values under, drawn once per process.
struct RedactionKeys {
    sip: SipHasher13,
    /// The base of the rolling hash, a polynomial in it modulo
    /// [`ROLLING_MODULUS`].
    base: u64,
}

impl RedactionKeys {
    fn rolling(&self, bytes: &[u8]) -> u64 {
        bytes
            .iter()
            .fold(0, |h, &b| add_mod(mul_mod(h, self.base), digit(b)))
    }

    /// The base raised to `exp`, the weight of a window's first byte.
    fn power(&self, exp: usize) -> u64 {
        (0..exp).fold(1, |p, _| mul_mod(p, self.base))
    }

    /// The rolling hash of the window one byte on, dropping `out` from its
    /// front, whose weight is `top`, and taking `next` on at its end.
    fn slide(&self, rolling: u64, top: u64, out: u8, next: u8) -> u64 {
        let dropped = add_mod(rolling, ROLLING_MODULUS - mul_mod(digit(out), top));
        add_mod(mul_mod(dropped, self.base), digit(next))
    }

    /// A rolling hash under the keyed hash, so a short value cannot be read
    /// back from it.
    fn seal(&self, rolling: u64) -> u64 {
        self.sip.hash(&rolling.to_le_bytes())
    }
}

/// A byte's digit in the rolling hash, never zero, so leading zero bytes
/// still count.
fn digit(b: u8) -> u64 {
    u64::from(b) + 1
}

/// `x` reduced once by the modulus: enough below twice the modulus.
fn reduce(x: u64) -> u64 {
    if x >= ROLLING_MODULUS {
        x - ROLLING_MODULUS
    } else {
        x
    }
}

fn add_mod(a: u64, b: u64) -> u64 {
    reduce(a + b)
}

fn mul_mod(a: u64, b: u64) -> u64 {
    let product = u128::from(a) * u128::from(b);
    let low = u64::try_from(product & u128::from(ROLLING_MODULUS)).expect("61 bits");
    let high = u64::try_from(product >> 61).expect("below 2^61 for reduced operands");
    // Each part is at most the modulus, so the sum is at most twice it.
    reduce(reduce(low + high))
}

fn redaction_keys() -> &'static RedactionKeys {
    static KEYS: OnceLock<RedactionKeys> = OnceLock::new();
    KEYS.get_or_init(|| {
        use rand::Rng;
        let mut rng = rand::rng();
        RedactionKeys {
            sip: SipHasher13::new_with_keys(rng.next_u64(), rng.next_u64()),
            base: 256 + rng.next_u64() % (ROLLING_MODULUS - 256),
        }
    })
}

impl SensitiveValues {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many distinct values are held.
    pub fn len(&self) -> usize {
        self.0.values().map(|h| h.bytes.len()).sum()
    }

    // spec: SEC#sensitive-value
    /// Hold `value`; an empty value is never sensitive.
    pub fn insert(&mut self, value: impl AsRef<[u8]>) {
        let value = value.as_ref();
        if value.is_empty() {
            return;
        }
        let keys = redaction_keys();
        let hashes = Arc::make_mut(&mut self.0).entry(value.len()).or_default();
        hashes.rolling.insert(keys.seal(keys.rolling(value)));
        hashes.bytes.insert(keys.sip.hash(value));
    }

    /// Hold every value `other` holds.
    pub fn extend(&mut self, other: &SensitiveValues) {
        if other.is_empty() {
            return;
        }
        if self.is_empty() {
            *self = other.clone();
            return;
        }
        let ours = Arc::make_mut(&mut self.0);
        for (len, hashes) in other.0.iter() {
            let held = ours.entry(*len).or_default();
            held.rolling.extend(&hashes.rolling);
            held.bytes.extend(&hashes.bytes);
        }
    }

    /// Call `found` with the byte range of each occurrence of a held value in
    /// `bytes`, until it breaks.
    ///
    /// Each length is one pass over `bytes`: the rolling hash moves along a
    /// window at a time, and only a window it matches is hashed in full.
    fn scan(&self, bytes: &[u8], mut found: impl FnMut(usize, usize) -> ControlFlow<()>) {
        let keys = redaction_keys();
        for (&len, hashes) in self.0.iter() {
            let Some(first) = bytes.get(..len) else {
                return;
            };
            let top = keys.power(len - 1);
            let mut rolling = keys.rolling(first);
            let mut start = 0;
            loop {
                if hashes.rolling.contains(&keys.seal(rolling))
                    && hashes
                        .bytes
                        .contains(&keys.sip.hash(&bytes[start..start + len]))
                    && found(start, start + len).is_break()
                {
                    return;
                }
                let Some(&next) = bytes.get(start + len) else {
                    break;
                };
                rolling = keys.slide(rolling, top, bytes[start], next);
                start += 1;
            }
        }
    }

    /// The byte ranges of `text` that an occurrence of a held value covers,
    /// until `found` breaks.
    ///
    /// A range that does not start and end on character boundaries is passed
    /// over: a held value that is valid UTF-8 always does wherever it occurs,
    /// so such a range is a Blob's bytes or a hash collision. So is one lying
    /// wholly inside a [`REDACTED`] marker already in the text, which shows
    /// the marker and nothing of the value, so that masking text twice leaves
    /// it as masking it once did, however short a held value is. A range that
    /// overlaps a marker without lying inside it is widened to cover the
    /// marker.
    fn occurrences(&self, text: &str, mut found: impl FnMut(usize, usize) -> ControlFlow<()>) {
        let markers: Vec<(usize, usize)> = text
            .match_indices(REDACTED)
            .map(|(at, m)| (at, at + m.len()))
            .collect();
        self.scan(text.as_bytes(), |mut start, mut end| {
            if !(text.is_char_boundary(start) && text.is_char_boundary(end)) {
                return ControlFlow::Continue(());
            }
            for &(m_start, m_end) in &markers {
                if m_start <= start && end <= m_end {
                    return ControlFlow::Continue(());
                }
                if start < m_end && m_start < end {
                    start = start.min(m_start);
                    end = end.max(m_end);
                }
            }
            found(start, end)
        });
    }

    // spec: SEC#redaction
    /// Whether `text` contains any held value.
    pub fn found_in(&self, text: &str) -> bool {
        let mut any = false;
        self.occurrences(text, |_, _| {
            any = true;
            ControlFlow::Break(())
        });
        any
    }

    /// Whether `bytes` contain any held value, wherever it starts.
    pub fn found_in_bytes(&self, bytes: &[u8]) -> bool {
        let mut any = false;
        self.scan(bytes, |_, _| {
            any = true;
            ControlFlow::Break(())
        });
        any
    }

    // spec: SEC#withheld-values
    /// `text` with each stretch an occurrence of a held value covers replaced
    /// by [`REDACTED`], and the rest as it is.
    ///
    /// A marker can join the text beside it into a new occurrence, so masking
    /// repeats until none is left; each repeat folds such an occurrence into
    /// its marker, so the text shrinks and the repeats end.
    pub fn mask<'t>(&self, text: &'t str) -> Cow<'t, str> {
        let mut masked = Cow::Borrowed(text);
        while let Some(next) = self.mask_once(&masked) {
            masked = Cow::Owned(next);
        }
        masked
    }

    fn mask_once(&self, text: &str) -> Option<String> {
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        self.occurrences(text, |start, end| {
            ranges.push((start, end));
            ControlFlow::Continue(())
        });
        if ranges.is_empty() {
            return None;
        }
        ranges.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
        for (start, end) in ranges {
            match merged.last_mut() {
                Some((_, last_end)) if start <= *last_end => *last_end = (*last_end).max(end),
                _ => merged.push((start, end)),
            }
        }
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        for (start, end) in merged {
            out.push_str(&text[at..start]);
            out.push_str(REDACTED);
            at = end;
        }
        out.push_str(&text[at..]);
        Some(out)
    }
}

impl SensitiveValues {
    // spec: SEC#redaction
    /// Whether any part of `value` contains a held value: a string, an
    /// object's key, or a number's or boolean's text, at any depth, or the
    /// JSON text of the whole, which holds a value a script supplied as other
    /// than a string. Each string is matched as it is, not as JSON escapes
    /// it, so a value holding a quote or a backslash is still found.
    pub fn found_in_json(&self, value: &serde_json::Value) -> bool {
        use serde_json::Value;
        match value {
            Value::String(s) => self.found_in(s),
            Value::Array(items) => {
                items.iter().any(|v| self.found_in_json(v)) || self.found_in(&value.to_string())
            }
            Value::Object(members) => {
                members
                    .iter()
                    .any(|(k, v)| self.found_in(k) || self.found_in_json(v))
                    || self.found_in(&value.to_string())
            }
            other => self.found_in(&other.to_string()),
        }
    }

    // spec: SEC#withheld-values
    /// `value` with each held value masked: in place in each string and
    /// object key, and a number or boolean that holds one replaced by its
    /// masked text. A whole that holds one only in its JSON text is replaced
    /// by that text masked.
    pub fn mask_json(&self, value: &serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        let masked = match value {
            Value::String(s) => return Value::String(self.mask(s).into_owned()),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.mask_json(v)).collect()),
            Value::Object(members) => Value::Object(
                members
                    .iter()
                    .map(|(k, v)| (self.mask(k).into_owned(), self.mask_json(v)))
                    .collect(),
            ),
            other => {
                let text = other.to_string();
                return if self.found_in(&text) {
                    Value::String(self.mask(&text).into_owned())
                } else {
                    other.clone()
                };
            }
        };
        let text = masked.to_string();
        if self.found_in(&text) {
            Value::String(self.mask(&text).into_owned())
        } else {
            masked
        }
    }
}

// spec: SEC#responses-and-events
/// Whether `member` is an identifier: a member that names or identifies
/// something rather than describing it, which the check of a whole response
/// or event leaves as it is.
pub fn is_identifier(member: &str) -> bool {
    const NAMES: &[&str] = &[
        "name",
        "app",
        "id",
        "kind",
        "type",
        "status",
        "state",
        "phase",
        "lifecycle",
        "health",
        "code",
        "trigger",
        "priority",
        "gate",
        "unit",
        "method",
        "timestamp",
        "generation",
    ];
    const SUFFIXES: &[&str] = &[
        "_id",
        "_name",
        "_kind",
        "_type",
        "_state",
        "_at",
        "_time",
        "_generation",
        "_hash",
        "_fingerprint",
    ];
    NAMES.contains(&member) || SUFFIXES.iter().any(|suffix| member.ends_with(suffix))
}

// spec: SEC#responses-and-events
/// Mask, in place, each string in `document` that contains a sensitive value
/// of the app it concerns, other than an identifier. `app` is the app the
/// whole concerns, if any; an object with an `app` member concerns that app
/// instead. `values_for` gives an app's sensitive values.
pub fn mask_document<E>(
    document: &mut serde_json::Value,
    app: Option<&str>,
    values_for: &mut impl FnMut(&str) -> Result<SensitiveValues, E>,
) -> Result<(), E> {
    use serde_json::Value;
    match document {
        Value::String(s) => {
            if let Some(app) = app
                && let Cow::Owned(masked) = values_for(app)?.mask(s)
            {
                *s = masked;
            }
        }
        Value::Array(items) => {
            for item in items {
                mask_document(item, app, values_for)?;
            }
        }
        Value::Object(members) => {
            let named = members
                .get("app")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let app = named.as_deref().or(app);
            for (member, value) in members.iter_mut() {
                if !is_identifier(member) {
                    mask_document(value, app, values_for)?;
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

impl fmt::Debug for SensitiveValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SensitiveValues({} withheld)", self.len())
    }
}

/// The sensitive values of one lifecycle operation or shell session: its
/// app's, the values of its secret params and of every invoked action's, and
/// the values its closures derive.
// spec: SEC#matching
#[derive(Clone)]
pub struct Scope(Arc<Mutex<SensitiveValues>>);

impl Scope {
    /// A scope starting from the sensitive values of the app it runs in.
    pub fn new(app: SensitiveValues) -> Self {
        Self(Arc::new(Mutex::new(app)))
    }

    /// A scope holding nothing yet.
    pub fn empty() -> Self {
        Self::new(SensitiveValues::default())
    }

    pub fn insert(&self, value: impl AsRef<[u8]>) {
        self.0.lock().insert(value);
    }

    /// Hold the value of each param in `params` that `is_secret` names, with
    /// a string held as it is and any other value as its JSON text.
    // spec: SEC#sensitive-value
    pub fn insert_secret_params<'p>(
        &self,
        params: impl IntoIterator<Item = (&'p String, &'p serde_json::Value)>,
        is_secret: impl Fn(&str) -> bool,
    ) {
        let mut values = self.0.lock();
        for (name, value) in params {
            if is_secret(name) {
                match value {
                    serde_json::Value::String(s) => values.insert(s),
                    other => values.insert(other.to_string()),
                }
            }
        }
    }

    pub fn values(&self) -> SensitiveValues {
        self.0.lock().clone()
    }
}

impl fmt::Debug for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Scope({:?})", self.0.lock())
    }
}

#[cfg(test)]
mod tests;
