use std::{collections::HashMap, sync::Arc};

use jiff::Timestamp;
use parking_lot::Mutex;
use seedling_protocol::actor::Actor;
use seedling_protocol::names::{AppName, SessionId};

use crate::runtime::barrier::CancelToken;

/// Tells a session to end. Cloned into whoever must notice a stop, whether
/// that is async code waiting for it or the script's blocking thread.
#[derive(Clone)]
pub(crate) struct StopSignal {
    cancel: Arc<CancelToken>,
    requested: Arc<tokio::sync::watch::Sender<bool>>,
}

impl StopSignal {
    pub(crate) fn new() -> Self {
        Self {
            cancel: Arc::new(CancelToken::new()),
            requested: Arc::new(tokio::sync::watch::channel(false).0),
        }
    }

    pub(crate) fn request(&self) {
        self.cancel.request();
        self.requested.send_replace(true);
    }

    pub(crate) fn is_requested(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Resolves once a stop has been requested, however long ago.
    pub(crate) async fn requested(&self) {
        let mut rx = self.requested.subscribe();
        let _ = rx.wait_for(|stopped| *stopped).await;
    }

    /// The same signal for code that blocks a thread rather than awaiting.
    pub(crate) fn cancel_token(&self) -> Arc<CancelToken> {
        Arc::clone(&self.cancel)
    }
}

pub struct ShellSession {
    pub session_id: SessionId,
    pub app: AppName,
    pub name: String,
    pub opened_at: Timestamp,
    pub actor: Option<Actor>,
    /// The podman container name (display_name) for this session, once its
    /// container exists.
    pub container_name: Option<String>,
    /// The terminal size to apply to the pty: the latest one asked for, which
    /// is applied when the pty comes into being if it has not yet.
    size: (u16, u16),
    /// `None` until the session's job has started.
    pty_master_fd: Option<std::os::unix::io::RawFd>,
    pub(crate) stop: StopSignal,
}

impl ShellSession {
    /// A session that is open but whose job has not started yet. It can be
    /// listed, resized and stopped from the moment it is registered.
    pub(crate) fn starting(
        session_id: SessionId,
        app: AppName,
        name: String,
        actor: Option<Actor>,
        size: (u16, u16),
        stop: StopSignal,
    ) -> Self {
        Self {
            session_id,
            app,
            name,
            opened_at: Timestamp::now(),
            actor,
            container_name: None,
            size,
            pty_master_fd: None,
            stop,
        }
    }
}

fn set_pty_size(fd: std::os::unix::io::RawFd, (rows, cols): (u16, u16)) {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: the caller holds the registry lock for a session whose pty is
    // still open, so `fd` is valid.
    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &ws) };
}

// i[shell.record]
pub struct ShellRecord {
    pub session_id: SessionId,
    pub app: AppName,
    pub name: String,
    pub opened_at: Timestamp,
    pub actor: Option<Actor>,
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

    /// Gives a starting session its job: records the container and pty, and
    /// puts the terminal size asked for so far on the pty. Returns `false` if
    /// the session is no longer registered, because it was stopped while its
    /// job was starting; the caller then owns tearing the job down.
    pub(crate) fn attach(
        &self,
        id: &SessionId,
        container_name: String,
        pty_master_fd: std::os::unix::io::RawFd,
    ) -> bool {
        let mut sessions = self.sessions.lock();
        let Some(session) = sessions.get_mut(id) else {
            return false;
        };
        set_pty_size(pty_master_fd, session.size);
        session.container_name = Some(container_name);
        session.pty_master_fd = Some(pty_master_fd);
        true
    }

    // i[shell.resize]
    pub fn resize(&self, id: &SessionId, rows: u16, cols: u16) -> bool {
        let mut sessions = self.sessions.lock();
        let Some(session) = sessions.get_mut(id) else {
            return false;
        };
        session.size = (rows, cols);
        if let Some(fd) = session.pty_master_fd {
            set_pty_size(fd, session.size);
        }
        true
    }

    // i[shell.stop]
    pub fn stop(&self, id: &SessionId) -> bool {
        let mut sessions = self.sessions.lock();
        let Some(session) = sessions.remove(id) else {
            return false;
        };
        session.stop.request();
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
            })
            .collect()
    }

    /// Returns the set of container names for all currently active shell sessions.
    /// Used by the reconciler to identify stray shell containers after a restart.
    pub fn active_container_names(&self) -> std::collections::HashSet<String> {
        self.sessions
            .lock()
            .values()
            .filter_map(|s| s.container_name.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starting(registry: &ShellRegistry, size: (u16, u16)) -> (SessionId, StopSignal) {
        let id = SessionId::generate();
        let stop = StopSignal::new();
        registry.insert(ShellSession::starting(
            id,
            AppName::new_unchecked("app"),
            "shell".to_owned(),
            None,
            size,
            stop.clone(),
        ));
        (id, stop)
    }

    /// A pty whose master and slave are both ours, so its size can be read back.
    fn pty() -> (i32, i32) {
        let (mut master, mut slave) = (0, 0);
        // SAFETY: both out-pointers are valid, the rest may be null.
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        assert_eq!(rc, 0, "openpty");
        (master, slave)
    }

    fn size_of(fd: i32) -> (u16, u16) {
        let mut ws = libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: `ws` is a valid winsize to write into.
        unsafe { libc::ioctl(fd, libc::TIOCGWINSZ as _, &mut ws) };
        (ws.ws_row, ws.ws_col)
    }

    // i[verify shell.startup]
    #[test]
    fn starting_session_is_listed_before_it_has_a_container() {
        let registry = ShellRegistry::new();
        let (id, _) = starting(&registry, (24, 80));

        let listed = registry.list(None);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, id);
        assert!(registry.active_container_names().is_empty());
    }

    // i[verify shell.startup]
    // i[verify shell.resize]
    #[test]
    fn resize_while_starting_is_applied_when_the_terminal_exists() {
        let registry = ShellRegistry::new();
        let (id, _) = starting(&registry, (24, 80));

        assert!(registry.resize(&id, 50, 132));
        assert!(registry.resize(&id, 40, 120));

        let (master, slave) = pty();
        assert!(registry.attach(&id, "container".to_owned(), master));
        assert_eq!(size_of(slave), (40, 120), "the latest size wins");
        assert_eq!(
            registry.active_container_names(),
            ["container".to_owned()].into()
        );

        assert!(registry.resize(&id, 10, 20));
        assert_eq!(size_of(slave), (10, 20), "later resizes reach the terminal");

        // SAFETY: both fds were opened above and are not used again.
        unsafe {
            libc::close(master);
            libc::close(slave);
        }
    }

    // i[verify shell.startup]
    #[test]
    fn initial_size_applies_when_nothing_resizes_it() {
        let registry = ShellRegistry::new();
        let (id, _) = starting(&registry, (30, 100));

        let (master, slave) = pty();
        assert!(registry.attach(&id, "container".to_owned(), master));
        assert_eq!(size_of(slave), (30, 100));

        // SAFETY: both fds were opened above and are not used again.
        unsafe {
            libc::close(master);
            libc::close(slave);
        }
    }

    // i[verify shell.startup]
    // i[verify shell.stop]
    #[tokio::test]
    async fn stop_while_starting_signals_and_refuses_the_late_attach() {
        let registry = ShellRegistry::new();
        let (id, stop) = starting(&registry, (24, 80));
        assert!(!stop.is_requested());

        assert!(registry.stop(&id));
        assert!(stop.is_requested());
        tokio::time::timeout(std::time::Duration::from_secs(1), stop.requested())
            .await
            .expect("a stop requested earlier is still seen");
        assert!(registry.list(None).is_empty());

        assert!(
            !registry.attach(&id, "container".to_owned(), -1),
            "a stopped session has nothing to attach to"
        );
        assert!(!registry.stop(&id));
        assert!(!registry.resize(&id, 1, 1));
    }
}
