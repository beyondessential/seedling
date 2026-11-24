//! The sensitive values the runtime withholds from operators, and how it
//! finds and masks them in what it reports.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashSet},
    fmt,
    sync::{Arc, OnceLock},
};

use parking_lot::Mutex;
use siphasher::sip::SipHasher13;

/// What an occurrence of a sensitive value in free text is replaced by.
// spec: SEC#withheld-values
pub const REDACTED: &str = "<redacted>";

/// A set of sensitive values, held only to find them in other text.
///
/// Each is held as its length and a keyed hash of its bytes: a fixed size
/// however long the value, and nothing a value could be read back from. A hash
/// that collides only withholds text that held no sensitive value. Copies of
/// a set share its storage until one is changed.
// spec: SEC#sensitive-value
#[derive(Clone, Default)]
pub struct SensitiveValues(Arc<BTreeMap<usize, HashSet<u64>>>);

/// The key the runtime hashes sensitive values under, drawn once per process.
fn redaction_hasher() -> &'static SipHasher13 {
    static HASHER: OnceLock<SipHasher13> = OnceLock::new();
    HASHER.get_or_init(|| {
        use rand::Rng;
        let mut rng = rand::rng();
        SipHasher13::new_with_keys(rng.next_u64(), rng.next_u64())
    })
}

impl SensitiveValues {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many distinct values are held.
    pub fn len(&self) -> usize {
        self.0.values().map(HashSet::len).sum()
    }

    // spec: SEC#sensitive-value
    /// Hold `value`; an empty value is never sensitive.
    pub fn insert(&mut self, value: impl AsRef<[u8]>) {
        let value = value.as_ref();
        if value.is_empty() {
            return;
        }
        let hash = redaction_hasher().hash(value);
        Arc::make_mut(&mut self.0)
            .entry(value.len())
            .or_default()
            .insert(hash);
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
            ours.entry(*len).or_default().extend(hashes);
        }
    }

    /// The byte ranges of `text` that an occurrence of a held value covers.
    ///
    /// A held value is valid UTF-8, so its bytes start and end on character
    /// boundaries wherever they occur; a range that does not can only come
    /// from a hash collision and is passed over.
    fn occurrences<'t>(&'t self, text: &'t str) -> impl Iterator<Item = (usize, usize)> + 't {
        let hasher = redaction_hasher();
        let bytes = text.as_bytes();
        self.0.iter().flat_map(move |(len, hashes)| {
            bytes
                .windows(*len)
                .enumerate()
                .filter(move |(_, window)| hashes.contains(&hasher.hash(window)))
                .map(move |(start, _)| (start, start + len))
                .filter(|(start, end)| text.is_char_boundary(*start) && text.is_char_boundary(*end))
        })
    }

    // spec: SEC#redaction
    /// Whether `text` contains any held value.
    pub fn found_in(&self, text: &str) -> bool {
        self.occurrences(text).next().is_some()
    }

    /// Whether `bytes` contain any held value, wherever it starts.
    pub fn found_in_bytes(&self, bytes: &[u8]) -> bool {
        let hasher = redaction_hasher();
        self.0.iter().any(|(len, hashes)| {
            bytes
                .windows(*len)
                .any(|window| hashes.contains(&hasher.hash(window)))
        })
    }

    // spec: SEC#withheld-values
    /// `text` with each stretch an occurrence of a held value covers replaced
    /// by [`REDACTED`], and the rest as it is.
    pub fn mask<'t>(&self, text: &'t str) -> Cow<'t, str> {
        let mut ranges: Vec<(usize, usize)> = self.occurrences(text).collect();
        if ranges.is_empty() {
            return Cow::Borrowed(text);
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
        Cow::Owned(out)
    }
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
                    other => values.insert(&other.to_string()),
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
