use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use orion::aead;
use secrecy::{ExposeSecret, SecretString};

// r[impl secret.key]
// r[impl secret.storage]
pub struct Cipher {
    key: aead::SecretKey,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Crypto(orion::errors::UnknownCryptoError),
    BadEncoding,
    InsecurePermissions { path: PathBuf, mode: u32 },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "key file I/O error: {e}"),
            Self::Crypto(_) => write!(f, "cipher operation failed"),
            Self::BadEncoding => write!(f, "key file contains invalid data"),
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

impl From<orion::errors::UnknownCryptoError> for Error {
    fn from(e: orion::errors::UnknownCryptoError) -> Self {
        Self::Crypto(e)
    }
}

impl Cipher {
    // r[impl secret.key]
    pub fn load_or_create(path: &Path) -> Result<Self, Error> {
        if path.exists() {
            let mode = path.metadata()?.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(Error::InsecurePermissions {
                    path: path.to_owned(),
                    mode,
                });
            }
            let bytes = std::fs::read(path)?;
            let key = aead::SecretKey::from_slice(&bytes)?;
            return Ok(Self { key });
        }

        let key = aead::SecretKey::default();
        std::fs::write(path, key.unprotected_as_bytes())?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Self { key })
    }

    // r[impl secret.storage]
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
        let key = aead::SecretKey::from_slice(&[42u8; 32]).expect("valid test key");
        Self { key }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // r[verify secret.storage]
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
            key: aead::SecretKey::from_slice(&[1u8; 32]).expect("valid key"),
        };
        assert!(other.decrypt(&ct).is_err());
    }

    // r[verify secret.key]
    #[test]
    fn keyfile_created_with_secure_perms() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.key");
        Cipher::load_or_create(&path).expect("create");
        let mode = path.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "key file must be 0600");
    }

    #[test]
    fn insecure_perms_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("loose.key");
        std::fs::write(&path, [0u8; 32]).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            Cipher::load_or_create(&path),
            Err(Error::InsecurePermissions { .. })
        ));
    }
}
