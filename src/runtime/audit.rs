use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio::{sync::broadcast, task::JoinHandle};
use tracing::{error, warn};

use crate::{oi::events::OiEvent, runtime::db::Db};

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
        serde_json::to_writer(&mut self.writer, event)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}

// r[impl audit.log.events]
pub fn spawn_audit_task(
    path: PathBuf,
    mut rx: broadcast::Receiver<OiEvent>,
    db: Arc<parking_lot::Mutex<Db>>,
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
                    let db = db.lock();
                    let _ = crate::runtime::faults::file_fault(
                        &db,
                        "seedling",
                        None,
                        None,
                        None,
                        "audit_lag",
                        &format!("audit log receiver lagged, {n} events dropped"),
                    );
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    })
}
