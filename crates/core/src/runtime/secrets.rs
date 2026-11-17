use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use orion::aead;

use secrecy::{ExposeSecret, SecretString};

use crate::runtime::db::Db;

// spec: SEC#secret-key
// spec: SEC#storage
pub struct Cipher {
    key: aead::SecretKey,
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
            let mode = path.metadata()?.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(Error::InsecurePermissions {
                    path: path.to_owned(),
                    mode,
                });
            }
            let bytes = std::fs::read(path)?;
            let key = aead::SecretKey::try_from(bytes.as_slice())?;
            return Ok(Self { key });
        }

        let key = aead::SecretKey::generate()?;
        // spec: INFRA#file-permissions
        seedling_protocol::keys::write_new_private_file(path, key.unprotected_as_ref())?;
        Ok(Self { key })
    }

    // spec: SEC#storage
    pub fn encrypt(&self, plaintext: &SecretString) -> Result<Vec<u8>, Error> {
        Ok(aead::seal(&self.key, plaintext.expose_secret().as_bytes())?)
    }

    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<SecretString, Error> {
        let plaintext = aead::open(&self.key, ciphertext)?;
        let s = String::from_utf8(plaintext).map_err(|_| Error::BadEncoding)?;
        Ok(SecretString::new(s.into()))
    }

    #[cfg(test)]
    pub fn for_tests() -> Self {
        let key = aead::SecretKey::try_from(&[42u8; 32]).expect("valid test key");
        Self { key }
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
        let other = Cipher {
            key: aead::SecretKey::try_from(&[1u8; 32]).expect("valid key"),
        };
        assert!(other.decrypt(&ct).is_err());
    }

    // spec: SEC#secret-key
    #[test]
    fn keyfile_created_with_secure_perms() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.key");
        Cipher::load_or_create(&path, &Db::open_in_memory().unwrap()).expect("create");
        let mode = path.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "key file must be 0600");
    }

    // spec: INFRA#file-permissions
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

    #[test]
    fn insecure_perms_rejected() {
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
