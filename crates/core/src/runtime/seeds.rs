//! The seeds that derived random values come from, and the frame through
//! which an evaluation's `std` helpers reach them.
//!
//! A `std` helper is a free function with no receiver, so the seeds an
//! evaluation derives from are installed on its thread for the length of the
//! evaluation, the way the action-closure and validator flags are.

use std::{cell::RefCell, fmt, sync::Arc};

use secrecy::{ExposeSecret, SecretBox, zeroize::Zeroize};

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
    static APP_SEED: RefCell<Option<AppSeed>> = const { RefCell::new(None) };
    static OPERATION_SEED: RefCell<Option<Seed>> = const { RefCell::new(None) };
    static DERIVED: RefCell<Option<DerivedValues>> = const { RefCell::new(None) };
}

/// The text forms of the values an evaluation derived from its app seed,
/// which the runtime withholds wherever it withholds secret parameter values.
// spec: SEC#redaction
#[derive(Clone, Default)]
pub struct DerivedValues(Vec<String>);

impl DerivedValues {
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

impl fmt::Debug for DerivedValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DerivedValues({} withheld)", self.0.len())
    }
}

/// Installs an app seed for the static and validator contexts of one
/// evaluation, and records what they derive; restores whatever it displaced
/// when dropped, so a nested evaluation leaves its caller's frame intact.
pub struct AppSeedFrame {
    previous_seed: Option<AppSeed>,
    previous_derived: Option<DerivedValues>,
}

impl AppSeedFrame {
    pub fn enter(seed: &AppSeed) -> Self {
        Self {
            previous_seed: APP_SEED.with(|s| s.replace(Some(seed.clone()))),
            previous_derived: DERIVED.with(|d| d.replace(Some(DerivedValues::default()))),
        }
    }

    /// The values derived from the app seed since the frame was entered.
    pub fn derived(&self) -> DerivedValues {
        DERIVED.with(|d| d.borrow().clone().unwrap_or_default())
    }
}

impl Drop for AppSeedFrame {
    fn drop(&mut self) {
        APP_SEED.with(|s| *s.borrow_mut() = self.previous_seed.take());
        DERIVED.with(|d| *d.borrow_mut() = self.previous_derived.take());
    }
}

/// Installs an operation seed for the action closures of one pass; restores
/// whatever it displaced when dropped.
pub struct OperationSeedFrame {
    previous: Option<Seed>,
}

impl OperationSeedFrame {
    pub fn enter(seed: &Seed) -> Self {
        Self {
            previous: OPERATION_SEED.with(|s| s.replace(Some(seed.clone()))),
        }
    }
}

impl Drop for OperationSeedFrame {
    fn drop(&mut self) {
        OPERATION_SEED.with(|s| *s.borrow_mut() = self.previous.take());
    }
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
pub fn record_derived(forms: impl IntoIterator<Item = String>) {
    DERIVED.with(|d| {
        if let Some(values) = d.borrow_mut().as_mut() {
            values.0.extend(forms.into_iter().filter(|f| !f.is_empty()));
        }
    });
}

#[cfg(test)]
mod tests;
