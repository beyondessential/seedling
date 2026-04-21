use std::{collections::HashMap, sync::Arc};

use jiff::Timestamp;
use parking_lot::Mutex;
use seedling_protocol::actor::Actor;
use seedling_protocol::names::AppName;
use uuid::Uuid;

pub type SessionId = Uuid;

pub struct ShellSession {
    pub session_id: SessionId,
    pub app: AppName,
    pub name: String,
    pub opened_at: Timestamp,
    pub actor: Option<Actor>,
    /// The podman container name (display_name) for this session.
    pub container_name: String,
    pub(crate) pty_master_fd: std::os::unix::io::RawFd,
    pub(crate) stop_tx: tokio::sync::oneshot::Sender<()>,
}

// i[shell.record]
pub struct ShellRecord {
    pub session_id: SessionId,
    pub app: AppName,
    pub name: String,
    pub opened_at: Timestamp,
    pub actor: Option<Actor>,
    pub container_name: String,
}

pub struct ShellRegistry {
    sessions: Mutex<HashMap<SessionId, ShellSession>>,
}

impl ShellRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sessions: Mutex::new(HashMap::new()),
        })
    }

    pub fn insert(&self, session: ShellSession) {
        self.sessions.lock().insert(session.session_id, session);
    }

    pub fn remove(&self, id: &SessionId) {
        self.sessions.lock().remove(id);
    }

    // i[shell.resize]
    pub fn resize(&self, id: &SessionId, rows: u16, cols: u16) -> bool {
        let sessions = self.sessions.lock();
        let Some(session) = sessions.get(id) else {
            return false;
        };
        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: pty_master_fd is valid while the session is registered.
        unsafe { libc::ioctl(session.pty_master_fd, libc::TIOCSWINSZ as _, &ws) };
        true
    }

    // i[shell.stop]
    pub fn stop(&self, id: &SessionId) -> bool {
        let mut sessions = self.sessions.lock();
        let Some(session) = sessions.remove(id) else {
            return false;
        };
        let _ = session.stop_tx.send(());
        true
    }

    // i[shell.list]
    pub fn list(&self, app: Option<&str>) -> Vec<ShellRecord> {
        self.sessions
            .lock()
            .values()
            .filter(|s| app.is_none_or(|a| s.app == a))
            .map(|s| ShellRecord {
                session_id: s.session_id,
                app: s.app.clone(),
                name: s.name.clone(),
                opened_at: s.opened_at,
                actor: s.actor.clone(),
                container_name: s.container_name.clone(),
            })
            .collect()
    }

    /// Returns the set of container names for all currently active shell sessions.
    /// Used by the reconciler to identify stray shell containers after a restart.
    pub fn active_container_names(&self) -> std::collections::HashSet<String> {
        self.sessions
            .lock()
            .values()
            .map(|s| s.container_name.clone())
            .collect()
    }
}
