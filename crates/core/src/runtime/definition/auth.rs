//! Registry credentials, read from the same files the host's container engine
//! reads, so a registry the host can pull images from serves definitions too.

use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::Deserialize;

/// Credentials for one registry, or none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credentials {
    Anonymous,
    Basic { username: String, password: String },
}

#[derive(Deserialize)]
struct AuthFile {
    #[serde(default)]
    auths: std::collections::BTreeMap<String, AuthEntry>,
}

#[derive(Deserialize)]
struct AuthEntry {
    #[serde(default)]
    auth: Option<String>,
}

/// The files searched for credentials, in the order the container engine
/// searches them (see containers-auth.json(5)).
// spec: DEFN#registry-access
pub fn auth_file_candidates() -> Vec<PathBuf> {
    if let Some(explicit) = std::env::var_os("REGISTRY_AUTH_FILE") {
        return vec![PathBuf::from(explicit)];
    }
    let home = home_dir();
    let mut out: Vec<PathBuf> = primary_auth_file(home.as_deref()).into_iter().collect();
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".config")));
    if let Some(config) = config_home {
        out.push(config.join("containers/auth.json"));
    }
    if let Some(home) = home {
        out.push(home.join(".docker/config.json"));
    }
    out
}

/// The container engine's own credentials file, which on Linux lives in the
/// user's runtime directory.
#[cfg(target_os = "linux")]
fn primary_auth_file(_home: Option<&Path>) -> Option<PathBuf> {
    Some(match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join("containers/auth.json"),
        None => PathBuf::from(format!(
            "/run/containers/{}/auth.json",
            rustix::process::getuid().as_raw()
        )),
    })
}

/// The container engine's own credentials file, which off Linux lives in the
/// user's configuration directory.
#[cfg(not(target_os = "linux"))]
fn primary_auth_file(home: Option<&Path>) -> Option<PathBuf> {
    home.map(|h| h.join(".config/containers/auth.json"))
}

/// The user's home directory: `HOME`, or on Windows, where that is seldom set,
/// the user's profile directory.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| {
            cfg!(windows)
                .then(|| std::env::var_os("USERPROFILE"))
                .flatten()
        })
        .map(PathBuf::from)
}

/// Credentials for `registry`/`repository` from the first file holding an
/// entry for it. Entries keyed by a repository path take precedence over
/// the registry's own entry, most specific first.
// spec: DEFN#registry-access
pub fn lookup(files: &[PathBuf], registry: &str, repository: &str) -> Credentials {
    for file in files {
        if let Some(creds) = lookup_in(file, registry, repository) {
            return creds;
        }
    }
    Credentials::Anonymous
}

fn lookup_in(file: &Path, registry: &str, repository: &str) -> Option<Credentials> {
    let text = std::fs::read_to_string(file).ok()?;
    let parsed: AuthFile = match serde_json::from_str(&text) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(file = %file.display(), "registry auth file is unreadable: {e}");
            return None;
        }
    };
    for key in candidate_keys(registry, repository) {
        let Some(entry) = parsed.auths.get(&key) else {
            continue;
        };
        let Some(encoded) = entry.auth.as_deref() else {
            continue;
        };
        let decoded = BASE64.decode(encoded).ok()?;
        let decoded = String::from_utf8(decoded).ok()?;
        let (username, password) = decoded.split_once(':')?;
        return Some(Credentials::Basic {
            username: username.to_owned(),
            password: password.to_owned(),
        });
    }
    None
}

fn candidate_keys(registry: &str, repository: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut path = repository;
    loop {
        keys.push(format!("{registry}/{path}"));
        match path.rsplit_once('/') {
            Some((parent, _)) => path = parent,
            None => break,
        }
    }
    keys.push(registry.to_owned());
    if registry == "docker.io" {
        keys.push("https://index.docker.io/v1/".to_owned());
        keys.push("index.docker.io".to_owned());
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    // spec: DEFN#registry-access
    #[test]
    fn most_specific_entry_in_the_first_file_wins() {
        let dir = tempfile::tempdir().unwrap();
        let first = write(
            dir.path(),
            "a.json",
            &format!(
                r#"{{"auths": {{
                    "ghcr.io": {{"auth": "{}"}},
                    "ghcr.io/org": {{"auth": "{}"}}
                }}}}"#,
                BASE64.encode("reg:one"),
                BASE64.encode("org:two"),
            ),
        );
        let second = write(
            dir.path(),
            "b.json",
            &format!(
                r#"{{"auths": {{"quay.io": {{"auth": "{}"}}}}}}"#,
                BASE64.encode("quay:three")
            ),
        );
        let files = vec![dir.path().join("missing.json"), first, second];
        assert_eq!(
            lookup(&files, "ghcr.io", "org/app-def"),
            Credentials::Basic {
                username: "org".into(),
                password: "two".into()
            }
        );
        assert_eq!(
            lookup(&files, "ghcr.io", "other/app"),
            Credentials::Basic {
                username: "reg".into(),
                password: "one".into()
            }
        );
        assert_eq!(
            lookup(&files, "quay.io", "x/y"),
            Credentials::Basic {
                username: "quay".into(),
                password: "three".into()
            }
        );
        assert_eq!(lookup(&files, "example.com", "x"), Credentials::Anonymous);
    }
}
