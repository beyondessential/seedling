use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use seedling_protocol::events::OiEvent;
use seedling_protocol::names::AppName;
use tokio::{sync::broadcast, task::JoinHandle};
use tracing::{error, warn};

use crate::runtime::db::DbHandle;

// r[impl audit.log]
pub struct AuditWriter {
    writer: BufWriter<File>,
    path: PathBuf,
    inode: u64,
}

impl AuditWriter {
    // r[impl audit.log.path]
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o750))?;
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)?;

        let inode = file.metadata()?.ino();

        Ok(Self {
            writer: BufWriter::new(file),
            path: path.to_owned(),
            inode,
        })
    }

    // r[impl audit.log.rotation]
    pub fn reopen_if_rotated(&mut self) -> io::Result<()> {
        let needs_reopen = match fs::metadata(&self.path) {
            Ok(meta) => meta.ino() != self.inode,
            Err(_) => true,
        };

        if needs_reopen {
            let new = Self::open(&self.path)?;
            self.writer = new.writer;
            self.inode = new.inode;
        }

        Ok(())
    }

    // r[impl audit.log.format]
    pub fn write_event(&mut self, event: &OiEvent) -> io::Result<()> {
        self.reopen_if_rotated()?;
        serde_json::to_writer(&mut self.writer, event).map_err(io::Error::other)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}

// r[impl audit.log.events]
pub fn spawn_audit_task(
    path: PathBuf,
    mut rx: broadcast::Receiver<OiEvent>,
    db: DbHandle,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut writer = match AuditWriter::open(&path) {
            Ok(w) => w,
            Err(e) => {
                error!(path = %path.display(), "failed to open audit log: {e}");
                return;
            }
        };

        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Err(e) = writer.write_event(&event) {
                        // r[impl audit.log.resilience]
                        error!(path = %path.display(), "failed to write audit event: {e}");
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    // r[impl audit.log.resilience]
                    warn!(dropped = n, "audit log receiver lagged, events lost");
                    db.call(move |db| {
                        let seedling_app = AppName::new_unchecked("seedling");
                        let _ = crate::runtime::faults::file_fault(
                            db,
                            &seedling_app,
                            None,
                            None,
                            None,
                            "audit_lag",
                            &format!("audit log receiver lagged, {n} events dropped"),
                        );
                    });
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn test_event(app: &str) -> OiEvent {
        OiEvent::AppRegistered {
            timestamp: jiff::Timestamp::now(),
            app: AppName::new_unchecked(app),
            generation: 1,
            actor: None,
        }
    }

    #[test]
    // r[verify audit.log.path]
    fn open_creates_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("a").join("b").join("audit.log");

        let _writer = AuditWriter::open(&nested).unwrap();

        assert!(nested.parent().unwrap().is_dir());
        assert!(nested.exists());
    }

    #[test]
    // r[verify audit.log.format]
    fn write_event_produces_json_line() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("audit.log");

        let mut writer = AuditWriter::open(&path).unwrap();
        writer.write_event(&test_event("testapp")).unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 1);

        let val: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(val["type"], "AppRegistered");
        assert_eq!(val["app"], "testapp");
        assert!(val["timestamp"].is_string());
    }

    #[test]
    // r[verify audit.log.rotation]
    fn reopen_after_rotation() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("audit.log");
        let rotated = tmp.path().join("audit.log.1");

        let mut writer = AuditWriter::open(&path).unwrap();
        writer.write_event(&test_event("before")).unwrap();

        fs::rename(&path, &rotated).unwrap();

        writer.write_event(&test_event("after")).unwrap();

        assert!(path.exists());
        let contents = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 1);

        let val: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(val["app"], "after");
    }

    #[test]
    fn write_multiple_events() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("audit.log");

        let mut writer = AuditWriter::open(&path).unwrap();
        writer.write_event(&test_event("app1")).unwrap();
        writer.write_event(&test_event("app2")).unwrap();
        writer.write_event(&test_event("app3")).unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 3);

        for line in &lines {
            let val: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(val["type"], "AppRegistered");
        }
    }
}
