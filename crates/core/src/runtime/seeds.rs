//! The seeds that derived random values come from, and the frame through
//! which an evaluation's `std` helpers reach them.
//!
//! A `std` helper is a free function with no receiver, so the seeds an
//! evaluation derives from are installed on its thread for the length of the
//! evaluation, the way the action-closure and validator flags are.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashSet},
    fmt,
    sync::{Arc, OnceLock},
};

use secrecy::{ExposeSecret, SecretBox, zeroize::Zeroize};
use siphasher::sip::SipHasher13;

use crate::runtime::secrets::{self, Cipher};

pub const SEED_LEN: usize = 32;

/// A seed, held so that it zeroes on drop and never prints.
#[derive(Clone)]
pub struct Seed(Arc<SecretBox<[u8; SEED_LEN]>>);

impl Seed {
    // spec: SEC#seeds
    /// A fresh seed from the operating system's secure random source.
    pub fn draw() -> Result<Self, orion::errors::UnknownCryptoError> {
        let mut bytes = Box::new([0u8; SEED_LEN]);
        orion::util::secure_rand_bytes(&mut bytes[..])?;
        Ok(Self(Arc::new(SecretBox::new(bytes))))
    }

    #[cfg(test)]
    pub fn for_tests() -> Self {
        Self::from_bytes([5; SEED_LEN])
    }

    #[cfg(test)]
    pub fn from_bytes(bytes: [u8; SEED_LEN]) -> Self {
        Self(Arc::new(SecretBox::new(Box::new(bytes))))
    }

    pub(crate) fn expose(&self) -> &[u8; SEED_LEN] {
        self.0.expose_secret()
    }

    // spec: SEC#seeds
    pub fn seal(&self, cipher: &Cipher) -> Result<Vec<u8>, secrets::Error> {
        cipher.encrypt_bytes(self.expose())
    }

    pub fn open(cipher: &Cipher, ciphertext: &[u8]) -> Result<Self, secrets::Error> {
        let mut plaintext = cipher.decrypt_bytes(ciphertext)?;
        let seed = <[u8; SEED_LEN]>::try_from(plaintext.as_slice())
            .map(|bytes| Self(Arc::new(SecretBox::new(Box::new(bytes)))))
            .map_err(|_| secrets::Error::BadEncoding);
        plaintext.zeroize();
        seed
    }
}

impl fmt::Debug for Seed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Seed(..)")
    }
}

/// An app's seed as the runtime holds it: the seed, or the reason it could
/// not be read.
///
/// An unreadable seed is kept as such, never mistaken for an app that has
/// none, so the runtime neither replaces it nor derives from anything else.
// spec: SEC#app-seeds
#[derive(Clone, Debug)]
pub enum AppSeed {
    Ready(Seed),
    Unreadable(Arc<str>),
}

impl AppSeed {
    // spec: SEC#app-seeds
    pub fn open(cipher: &Cipher, ciphertext: &[u8]) -> Self {
        match Seed::open(cipher, ciphertext) {
            Ok(seed) => Self::Ready(seed),
            Err(e) => Self::Unreadable(format!("{e:?}").into()),
        }
    }

    #[cfg(test)]
    pub fn for_tests() -> Self {
        Self::Ready(Seed::from_bytes([9; SEED_LEN]))
    }

    /// A seed for an evaluation whose derived values go nowhere, such as a
    /// preview of a definition that is not registered.
    pub fn throwaway() -> Self {
        match Seed::draw() {
            Ok(seed) => Self::Ready(seed),
            Err(e) => Self::Unreadable(format!("no seed could be drawn: {e:?}").into()),
        }
    }
}

thread_local! {
    static DERIVATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PASSWORD_HASHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static APP_SEED: RefCell<Option<AppSeed>> = const { RefCell::new(None) };
    static OPERATION_SEED: RefCell<Option<Seed>> = const { RefCell::new(None) };
    static DERIVED: RefCell<Option<DerivedValues>> = const { RefCell::new(None) };
    static WITHHOLDING: RefCell<Option<Withholding>> = const { RefCell::new(None) };
}

/// Text forms held as their length and a keyed hash of their bytes: a fixed
/// size however long the value, and nothing a value could be read back from.
/// A hash that collides only withholds text that held no such value.
#[derive(Clone, Default)]
struct Forms(BTreeMap<usize, HashSet<u64>>);

impl Forms {
    fn insert(&mut self, form: &[u8]) {
        if form.is_empty() {
            return;
        }
        let hash = redaction_hasher().hash(form);
        self.0.entry(form.len()).or_default().insert(hash);
    }

    fn extend(&mut self, other: Forms) {
        for (len, hashes) in other.0 {
            self.0.entry(len).or_default().extend(hashes);
        }
    }

    fn len(&self) -> usize {
        self.0.values().map(HashSet::len).sum()
    }

    fn found_in(&self, bytes: &[u8]) -> bool {
        let hasher = redaction_hasher();
        self.0.iter().any(|(len, hashes)| {
            bytes
                .windows(*len)
                .any(|window| hashes.contains(&hasher.hash(window)))
        })
    }
}

/// The text forms of the values an evaluation derived from its app seed, and
/// of the helper results it withholds as derived values, which the runtime
/// withholds wherever it withholds secret parameter values.
///
/// They are needed only to find them in other text, so they are held as
/// [`Forms`]. The set is shared by every copy of the definition that carries
/// it.
// spec: SEC#redaction
#[derive(Clone, Default)]
pub struct DerivedValues(Arc<Forms>);

/// The key the runtime hashes derived values under, drawn once per process.
fn redaction_hasher() -> &'static SipHasher13 {
    static HASHER: OnceLock<SipHasher13> = OnceLock::new();
    HASHER.get_or_init(|| {
        use rand::Rng;
        let mut rng = rand::rng();
        SipHasher13::new_with_keys(rng.next_u64(), rng.next_u64())
    })
}

impl DerivedValues {
    pub fn is_empty(&self) -> bool {
        self.0.0.is_empty()
    }

    /// How many distinct forms are held.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    fn insert(&mut self, form: &[u8]) {
        if !form.is_empty() {
            Arc::make_mut(&mut self.0).insert(form);
        }
    }

    // spec: SEC#redaction
    /// Whether `text` contains any of the forms held.
    pub fn found_in(&self, text: &str) -> bool {
        self.found_in_bytes(text.as_bytes())
    }

    /// Whether `bytes` contain any of the forms held.
    pub fn found_in_bytes(&self, bytes: &[u8]) -> bool {
        self.0.found_in(bytes)
    }
}

/// What the helper results of one evaluation may have to be withheld for.
///
/// Whether a parameter is secret is settled only when the evaluation ends,
/// since a script can declare its flag after reading its value, so a result
/// computed from a parameter's value waits here until then.
// spec: SEC#redaction
#[derive(Default)]
struct Withholding {
    /// The stored values the script has read, by parameter.
    params: BTreeMap<String, Forms>,
    /// Results computed from those values, by the parameters they depend on,
    /// with how many results each holds.
    pending: BTreeMap<BTreeSet<String>, (Forms, usize)>,
}

/// Why a helper result may be withheld.
pub enum Dependence {
    /// An argument contains a derived value, so the result is withheld.
    Derived,
    /// An argument contains the value of one of these parameters, so the
    /// result is withheld if any of them is secret when the evaluation ends.
    Params(BTreeSet<String>),
}

impl fmt::Debug for DerivedValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DerivedValues({} withheld)", self.len())
    }
}

/// Installs an app seed for the static and validator contexts of one
/// evaluation, and records what they derive; restores whatever it displaced
/// when dropped, so a nested evaluation leaves its caller's frame intact.
pub struct AppSeedFrame {
    previous_seed: Option<AppSeed>,
    previous_derived: Option<DerivedValues>,
    previous_withholding: Option<Withholding>,
    previous_derivations: usize,
    previous_password_hashes: usize,
}

impl AppSeedFrame {
    pub fn enter(seed: &AppSeed) -> Self {
        Self {
            previous_seed: APP_SEED.with(|s| s.replace(Some(seed.clone()))),
            previous_derived: DERIVED.with(|d| d.replace(Some(DerivedValues::default()))),
            previous_withholding: WITHHOLDING
                .with(|w| w.replace(Some(Withholding::default()))),
            previous_derivations: DERIVATIONS.with(|c| c.replace(0)),
            previous_password_hashes: PASSWORD_HASHES.with(|c| c.replace(0)),
        }
    }

    // spec: SEC#redaction
    /// Take the values derived from the app seed since the frame was entered,
    /// with every result computed from a parameter that `is_secret` now says
    /// is secret.
    // spec: LIM#limits
    /// Each such result counts toward `limit`, 0 being no limit; an error
    /// says the evaluation went beyond it.
    pub fn take_derived(
        &self,
        is_secret: impl Fn(&str) -> bool,
        limit: usize,
    ) -> Result<DerivedValues, String> {
        let mut derived = DERIVED.with(|d| {
            d.borrow_mut()
                .as_mut()
                .map(std::mem::take)
                .unwrap_or_default()
        });
        let pending = WITHHOLDING.with(|w| {
            w.borrow_mut()
                .as_mut()
                .map(|w| std::mem::take(&mut w.pending))
                .unwrap_or_default()
        });
        for (params, (forms, results)) in pending {
            if !params.iter().any(|p| is_secret(p)) {
                continue;
            }
            for _ in 0..results {
                if !count_derivation(limit) {
                    return Err("Number of derived values too large".into());
                }
            }
            Arc::make_mut(&mut derived.0).extend(forms);
        }
        Ok(derived)
    }
}

impl Drop for AppSeedFrame {
    fn drop(&mut self) {
        APP_SEED.with(|s| *s.borrow_mut() = self.previous_seed.take());
        DERIVED.with(|d| *d.borrow_mut() = self.previous_derived.take());
        WITHHOLDING.with(|w| *w.borrow_mut() = self.previous_withholding.take());
        DERIVATIONS.with(|c| c.set(self.previous_derivations));
        PASSWORD_HASHES.with(|c| c.set(self.previous_password_hashes));
    }
}

/// Installs an operation seed for the action closures of one pass; restores
/// whatever it displaced when dropped.
pub struct OperationSeedFrame {
    previous: Option<Seed>,
    previous_derivations: usize,
    previous_password_hashes: usize,
}

impl OperationSeedFrame {
    pub fn enter(seed: &Seed) -> Self {
        Self {
            previous: OPERATION_SEED.with(|s| s.replace(Some(seed.clone()))),
            previous_derivations: DERIVATIONS.with(|c| c.replace(0)),
            previous_password_hashes: PASSWORD_HASHES.with(|c| c.replace(0)),
        }
    }
}

impl Drop for OperationSeedFrame {
    fn drop(&mut self) {
        OPERATION_SEED.with(|s| *s.borrow_mut() = self.previous.take());
        DERIVATIONS.with(|c| c.set(self.previous_derivations));
        PASSWORD_HASHES.with(|c| c.set(self.previous_password_hashes));
    }
}

/// Count one more in `counter`, saying whether it is within `limit`, 0 being
/// no limit; one past it is not counted.
fn count(counter: &'static std::thread::LocalKey<std::cell::Cell<usize>>, limit: usize) -> bool {
    counter.with(|c| {
        let n = c.get() + 1;
        let within = limit == 0 || n <= limit;
        if within {
            c.set(n);
        }
        within
    })
}

// spec: LIM#limits
/// Count one more derived value in the current frame, saying whether it is
/// within `limit`, 0 being no limit; one past it is not counted.
pub fn count_derivation(limit: usize) -> bool {
    count(&DERIVATIONS, limit)
}

// spec: LIM#limits
/// Count one more password hash in the current frame, as
/// [`count_derivation`] counts derived values.
pub fn count_password_hash(limit: usize) -> bool {
    count(&PASSWORD_HASHES, limit)
}

/// The seed installed for the static and validator contexts.
pub fn app_seed() -> Result<Seed, String> {
    APP_SEED.with(|s| match &*s.borrow() {
        Some(AppSeed::Ready(seed)) => Ok(seed.clone()),
        Some(AppSeed::Unreadable(reason)) => {
            Err(format!("the app's seed cannot be read: {reason}"))
        }
        None => Err("no app seed is available to this evaluation".into()),
    })
}

/// The seed installed for the action context.
pub fn operation_seed() -> Result<Seed, String> {
    OPERATION_SEED.with(|s| {
        s.borrow()
            .clone()
            .ok_or_else(|| "no operation seed is available to this action closure".into())
    })
}

/// Record a value derived from the app seed, in each text form a script can
/// turn it into.
pub fn record_derived(forms: impl IntoIterator<Item = impl AsRef<[u8]>>) {
    DERIVED.with(|d| {
        if let Some(values) = d.borrow_mut().as_mut() {
            for form in forms {
                values.insert(form.as_ref());
            }
        }
    });
}

// spec: SEC#redaction
/// Record a parameter's stored value as the script reads it, so a helper
/// result computed from it can be withheld if the parameter is secret.
pub fn record_param_value(name: &str, value: &str) {
    WITHHOLDING.with(|w| {
        if let Some(w) = w.borrow_mut().as_mut() {
            w.params
                .entry(name.to_owned())
                .or_default()
                .insert(value.as_bytes());
        }
    });
}

// spec: SEC#redaction
/// What a helper result computed from `args` may be withheld for, if
/// anything: an argument contains a withheld value when it holds a derived
/// value, a parameter's value, or an earlier result computed from one.
pub fn dependence(args: &[&[u8]]) -> Option<Dependence> {
    let derived = DERIVED.with(|d| {
        d.borrow()
            .as_ref()
            .is_some_and(|d| args.iter().any(|a| d.found_in_bytes(a)))
    });
    if derived {
        return Some(Dependence::Derived);
    }
    let params = WITHHOLDING.with(|w| {
        let w = w.borrow();
        let Some(w) = w.as_ref() else {
            return BTreeSet::new();
        };
        let found = |forms: &Forms| args.iter().any(|a| forms.found_in(a));
        let mut params: BTreeSet<String> = w
            .params
            .iter()
            .filter(|(_, forms)| found(forms))
            .map(|(name, _)| name.clone())
            .collect();
        for (names, (forms, _)) in &w.pending {
            if found(forms) {
                params.extend(names.iter().cloned());
            }
        }
        params
    });
    (!params.is_empty()).then_some(Dependence::Params(params))
}

// spec: SEC#redaction
/// Withhold a helper result, in each of its text `forms`, for `dependence`.
// spec: LIM#limits
/// A result withheld as a derived value counts toward `limit` at once; one
/// waiting on a parameter's flag counts if it is withheld when the
/// evaluation ends. Says whether the result is within the limit.
pub fn withhold(
    dependence: Dependence,
    forms: impl IntoIterator<Item = impl AsRef<[u8]>>,
    limit: usize,
) -> bool {
    match dependence {
        Dependence::Derived => {
            if !count_derivation(limit) {
                return false;
            }
            record_derived(forms);
        }
        Dependence::Params(params) => WITHHOLDING.with(|w| {
            if let Some(w) = w.borrow_mut().as_mut() {
                let (held, results) = w.pending.entry(params).or_default();
                for form in forms {
                    held.insert(form.as_ref());
                }
                *results += 1;
            }
        }),
    }
    true
}

#[cfg(test)]
mod tests;
