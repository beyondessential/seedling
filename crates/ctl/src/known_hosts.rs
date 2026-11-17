use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};

pub enum Status {
    Match,
    Unknown,
    Mismatch { expected: String },
}

pub struct KnownHosts {
    path: PathBuf,
    entries: HashMap<String, String>,
}

impl KnownHosts {
    // spec: LINUX#per-user-state
    pub fn default_path() -> PathBuf {
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("seedling")
            .join("known_hosts")
    }

    /// Read the file, which is empty when it does not exist. Any other failure
    /// to read it is an error, never an empty set of entries.
    // spec: CLI#server-trust
    pub fn load(path: &Path) -> io::Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let mut entries = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.splitn(2, ' ');
            if let (Some(ep), Some(fp)) = (parts.next(), parts.next()) {
                entries.insert(ep.to_owned(), fp.to_owned());
            }
        }
        Ok(Self {
            path: path.to_owned(),
            entries,
        })
    }

    pub fn check(&self, endpoint: &str, fingerprint: &str) -> Status {
        match self.entries.get(endpoint) {
            Some(saved) if saved == fingerprint => Status::Match,
            Some(saved) => Status::Mismatch {
                expected: saved.clone(),
            },
            None => Status::Unknown,
        }
    }

    pub fn add(&mut self, endpoint: &str, fingerprint: &str) {
        self.entries
            .insert(endpoint.to_owned(), fingerprint.to_owned());
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = String::from("# seedling-ctl known hosts\n# endpoint sha256-fingerprint\n");
        let mut pairs: Vec<_> = self.entries.iter().collect();
        pairs.sort_by_key(|(ep, _)| ep.as_str());
        for (ep, fp) in pairs {
            out.push_str(ep);
            out.push(' ');
            out.push_str(fp);
            out.push('\n');
        }
        std::fs::write(&self.path, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: CLI#server-trust
    #[test]
    fn a_missing_file_has_no_entries() {
        let dir = tempfile::tempdir().unwrap();
        let kh = KnownHosts::load(&dir.path().join("known_hosts")).expect("a missing file loads");
        assert!(matches!(kh.check("[::1]:7891", "fp"), Status::Unknown));
    }

    // spec: CLI#server-trust
    #[test]
    fn a_file_that_cannot_be_read_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_dir = dir.path().join("state");
        std::fs::write(&not_a_dir, "").unwrap();
        assert!(KnownHosts::load(&not_a_dir.join("known_hosts")).is_err());
        assert!(KnownHosts::load(dir.path()).is_err());
    }
}
