use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};

pub(super) enum Status {
    Match,
    Unknown,
    Mismatch { expected: String },
}

pub(super) struct KnownHosts {
    path: PathBuf,
    entries: HashMap<String, String>,
}

impl KnownHosts {
    pub(super) fn default_path() -> PathBuf {
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("seedling")
            .join("known_hosts")
    }

    pub(super) fn load(path: &Path) -> io::Result<Self> {
        let mut entries = HashMap::new();
        if path.exists() {
            for line in std::fs::read_to_string(path)?.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let mut parts = line.splitn(2, ' ');
                if let (Some(ep), Some(fp)) = (parts.next(), parts.next()) {
                    entries.insert(ep.to_owned(), fp.to_owned());
                }
            }
        }
        Ok(Self {
            path: path.to_owned(),
            entries,
        })
    }

    pub(super) fn empty(path: PathBuf) -> Self {
        Self {
            path,
            entries: HashMap::new(),
        }
    }

    pub(super) fn check(&self, endpoint: &str, fingerprint: &str) -> Status {
        match self.entries.get(endpoint) {
            Some(saved) if saved == fingerprint => Status::Match,
            Some(saved) => Status::Mismatch {
                expected: saved.clone(),
            },
            None => Status::Unknown,
        }
    }

    pub(super) fn add(&mut self, endpoint: &str, fingerprint: &str) {
        self.entries
            .insert(endpoint.to_owned(), fingerprint.to_owned());
    }

    pub(super) fn save(&self) -> io::Result<()> {
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
