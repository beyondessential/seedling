use std::path::{Path, PathBuf};

use orion::{
    aead, auth,
    hazardous::kdf::hkdf::{Hkdf, SHA512},
};

use secrecy::{ExposeSecret, SecretString};

use crate::runtime::db::Db;

// spec: SEC#secret-key
// spec: SEC#storage
pub struct Cipher {
    key: aead::SecretKey,
    digest_key: auth::SecretKey,
}

/// The context the key digests' subkey is derived under, so that no other use
/// of the secret key can produce the same key.
const KEY_DIGEST_CONTEXT: &[u8] = b"seedling gate key digest v1";

/// A digest of a gate key under the secret key, which tells keys apart
/// without holding them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyDigest([u8; KeyDigest::LEN]);

impl KeyDigest {
    pub const LEN: usize = 32;

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The digest `bytes` holds, or `None` where they are not the length of
    /// one, as a damaged record's would not be.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Crypto(orion::errors::UnknownCryptoError),
    BadEncoding,
    InsecurePermissions {
        path: PathBuf,
        mode: u32,
    },
    /// The key file is gone while the database holds values protected under it.
    MissingKey {
        path: PathBuf,
    },
    /// Whether the database holds protected values could not be read.
    Database(rusqlite::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "key file I/O error: {e}"),
            Self::Crypto(_) => write!(f, "cipher operation failed"),
            Self::BadEncoding => write!(f, "key file contains invalid data"),
            Self::MissingKey { path } => write!(
                f,
                "key file {} is missing, and the database holds values protected under it; \
                 restore the file it was created with",
                path.display()
            ),
            Self::Database(e) => write!(
                f,
                "cannot tell whether the database holds values protected under the secret key: {e}"
            ),
            Self::InsecurePermissions { path, mode } => write!(
                f,
                "key file {} has insecure permissions (0{:o}); expected 0600",
                path.display(),
                mode,
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Database(e)
    }
}

impl From<orion::errors::UnknownCryptoError> for Error {
    fn from(e: orion::errors::UnknownCryptoError) -> Self {
        Self::Crypto(e)
    }
}

impl Cipher {
    // spec: SEC#secret-key
    /// Load the key file, or create it when there is none and nothing in `db`
    /// is protected under a key. A missing file beside protected values is an
    /// error: a fresh key could never open them, and would let the runtime
    /// start as though they were not there.
    pub fn load_or_create(path: &Path, db: &Db) -> Result<Self, Error> {
        let missing = matches!(
            std::fs::symlink_metadata(path),
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound
        );
        if missing && protected_values_exist(db)? {
            return Err(Error::MissingKey {
                path: path.to_owned(),
            });
        }
        if !missing {
            if let Some(mode) = crate::runtime::private_files::exposed_mode(path)? {
                return Err(Error::InsecurePermissions {
                    path: path.to_owned(),
                    mode,
                });
            }
            let bytes = std::fs::read(path)?;
            let key = aead::SecretKey::try_from(bytes.as_slice())?;
            return Self::from_key(key);
        }

        let key = aead::SecretKey::generate()?;
        // spec: INFRA#file-permissions
        seedling_protocol::keys::write_new_private_file(path, key.unprotected_as_ref())?;
        Self::from_key(key)
    }

    fn from_key(key: aead::SecretKey) -> Result<Self, Error> {
        let mut subkey = [0u8; 32];
        Hkdf::<SHA512>::derive_key(
            &[],
            key.unprotected_as_ref(),
            Some(KEY_DIGEST_CONTEXT),
            &mut subkey,
        )?;
        let digest_key = auth::SecretKey::try_from(&subkey[..])?;
        Ok(Self { key, digest_key })
    }

    /// The digest of `canonical` under a subkey derived from the secret key.
    ///
    /// The same bytes digest the same under one secret key and differently
    /// under another, and the digest holds nothing the bytes could be
    /// recovered from.
    ///
    /// orion's `auth` is keyed BLAKE2b-256, so its tag is exactly
    /// [`KeyDigest::LEN`] bytes.
    // spec: GRUN#outcome
    pub fn key_digest(&self, canonical: &[u8]) -> Result<KeyDigest, Error> {
        let tag = auth::authenticate(&self.digest_key, canonical)?;
        KeyDigest::from_bytes(tag.unprotected_as_ref()).ok_or(Error::BadEncoding)
    }

    // spec: SEC#storage
    pub fn encrypt(&self, plaintext: &SecretString) -> Result<Vec<u8>, Error> {
        Ok(aead::seal(&self.key, plaintext.expose_secret().as_bytes())?)
    }

    // spec: SEC#seeds
    pub fn encrypt_bytes(&self, plaintext: &[u8]) -> Result<Vec<u8>, Error> {
        Ok(aead::seal(&self.key, plaintext)?)
    }

    pub fn decrypt_bytes(&self, ciphertext: &[u8]) -> Result<Vec<u8>, Error> {
        Ok(aead::open(&self.key, ciphertext)?)
    }

    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<SecretString, Error> {
        let plaintext = aead::open(&self.key, ciphertext)?;
        let s = String::from_utf8(plaintext).map_err(|_| Error::BadEncoding)?;
        Ok(SecretString::new(s.into()))
    }

    #[cfg(test)]
    pub fn for_tests() -> Self {
        Self::for_tests_with(&[42u8; 32])
    }

    #[cfg(test)]
    pub fn for_tests_with(key: &[u8; 32]) -> Self {
        let key = aead::SecretKey::try_from(key).expect("valid test key");
        Self::from_key(key).expect("a key derives its subkey")
    }
}

/// Whether any column of the database that holds a value protected under the
/// secret key holds one.
///
/// Every such column is named `…ciphertext`, so the tables and columns are
/// read from the schema rather than listed here, and a table added later is
/// covered without anyone remembering to add it.
// spec: SEC#secret-key
pub fn protected_values_exist(db: &Db) -> rusqlite::Result<bool> {
    let tables: Vec<String> = {
        let mut stmt = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?;
        stmt.query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for table in tables {
        let columns: Vec<String> = {
            let mut stmt = db
                .conn
                .prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
            stmt.query_map([], |row| row.get::<_, String>(1))?
                .collect::<rusqlite::Result<_>>()?
        };
        for column in columns.iter().filter(|c| c.ends_with("ciphertext")) {
            let held: bool = db.conn.query_row(
                &format!(
                    "SELECT EXISTS (SELECT 1 FROM \"{table}\" WHERE \"{column}\" IS NOT NULL)"
                ),
                [],
                |row| row.get(0),
            )?;
            if held {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: SEC#storage
    #[test]
    fn round_trip() {
        let cipher = Cipher::for_tests();
        let plaintext = SecretString::new("hunter2".into());
        let ct = cipher.encrypt(&plaintext).expect("encrypt");
        let got = cipher.decrypt(&ct).expect("decrypt");
        assert_eq!(got.expose_secret(), "hunter2");
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let cipher = Cipher::for_tests();
        let mut ct = cipher
            .encrypt(&SecretString::new("secret".into()))
            .expect("encrypt");
        let last = ct.len() - 1;
        ct[last] ^= 0xff;
        assert!(cipher.decrypt(&ct).is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let cipher = Cipher::for_tests();
        let ct = cipher
            .encrypt(&SecretString::new("secret".into()))
            .expect("encrypt");
        let other = Cipher::for_tests_with(&[1u8; 32]);
        assert!(other.decrypt(&ct).is_err());
    }

    // spec: GRUN#outcome
    #[test]
    fn a_key_digest_is_stable_for_one_secret_key_and_differs_across_secret_keys() {
        let first = Cipher::for_tests_with(&[1u8; 32]);
        let again = Cipher::for_tests_with(&[1u8; 32]);
        let other = Cipher::for_tests_with(&[2u8; 32]);
        let bytes = b"{\"version\":\"1.2.3\"}";

        let digest = first.key_digest(bytes).expect("digests");
        assert_eq!(digest, again.key_digest(bytes).expect("digests"));
        assert_ne!(digest, other.key_digest(bytes).expect("digests"));
        assert_ne!(digest, first.key_digest(b"other").expect("digests"));
    }

    // spec: GRUN#outcome
    #[test]
    fn a_key_digest_is_not_a_plain_hash_of_the_key() {
        use sha2::{Digest, Sha256};
        let digest = Cipher::for_tests().key_digest(b"1").expect("digests");
        assert_eq!(digest.as_bytes().len(), KeyDigest::LEN);
        assert_ne!(digest.as_bytes(), Sha256::digest(b"1").as_slice());
    }

    // spec: GRUN#outcome
    #[test]
    fn a_digest_round_trips_through_its_bytes_and_a_wrong_length_is_refused() {
        let digest = Cipher::for_tests().key_digest(b"1").expect("digests");
        assert_eq!(KeyDigest::from_bytes(digest.as_bytes()), Some(digest));
        assert_eq!(KeyDigest::from_bytes(&digest.as_bytes()[1..]), None);
    }

    // spec: SEC#secret-key
    #[cfg(unix)]
    #[test]
    fn keyfile_created_with_secure_perms() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.key");
        Cipher::load_or_create(&path, &Db::open_in_memory().unwrap()).expect("create");
        let mode = path.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "key file must be 0600");
    }

    // spec: INFRA#file-permissions
    #[cfg(unix)]
    #[test]
    fn keyfile_creation_does_not_follow_a_planted_symlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.key");
        let target = dir.path().join("elsewhere");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(Cipher::load_or_create(&path, &Db::open_in_memory().unwrap()).is_err());
        assert!(
            !target.exists(),
            "the key must not be written through the link"
        );
    }

    #[cfg(unix)]
    #[test]
    fn insecure_perms_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("loose.key");
        std::fs::write(&path, [0u8; 32]).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            Cipher::load_or_create(&path, &Db::open_in_memory().unwrap()),
            Err(Error::InsecurePermissions { .. })
        ));
    }

    // spec: SEC#secret-key
    #[test]
    fn a_missing_key_file_beside_protected_values_stops_startup_naming_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("seedling.db.key");
        let db = Db::open_in_memory().unwrap();
        let first = Cipher::load_or_create(&path, &db).expect("a fresh database creates a key");
        let app = seedling_protocol::names::AppName::new("demo").unwrap();
        crate::runtime::apps::secret_params::upsert_secret_param(
            &db,
            &first,
            &app,
            &seedling_protocol::names::ParamName::new("token").unwrap(),
            &SecretString::new("hunter2".into()),
        )
        .unwrap();

        std::fs::remove_file(&path).unwrap();
        let err = Cipher::load_or_create(&path, &db)
            .err()
            .expect("must not start");
        assert!(
            matches!(&err, Error::MissingKey { path: p } if p == &path),
            "{err}"
        );
        assert!(err.to_string().contains("seedling.db.key"));
        assert!(!path.exists(), "a fresh key must not be created");
    }
}
