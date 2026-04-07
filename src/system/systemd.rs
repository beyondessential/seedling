use std::time::Duration;

use snafu::Snafu;

use crate::system::{
    ProcessManager,
    types::{TransientUnitSpec, UnitState, UnitSummary},
};

// ---------------------------------------------------------------------------
// Internal error type
// ---------------------------------------------------------------------------

#[derive(Debug, Snafu)]
pub(crate) enum SystemdError {
    #[snafu(display("D-Bus error: {message}"))]
    DBus { message: String },
    #[snafu(display("unit {name} did not stop within the timeout"))]
    WaitTimeout { name: String },
    #[snafu(display("I/O error writing unit file: {source}"))]
    Io { source: std::io::Error },
}

// ---------------------------------------------------------------------------
// SystemdManager
// ---------------------------------------------------------------------------

/// `ProcessManager` implementation backed by the systemd system D-Bus
/// interface (`org.freedesktop.systemd1.Manager`) via `zbus`.
///
/// One `zbus::Connection` is created at startup and held for the process
/// lifetime. Transient units are started via `StartTransientUnit`. Persistent
/// unit files are written to `/etc/systemd/system/`.
///
/// All methods are currently stubs; implement incrementally starting with
/// `start_transient` and `wait_unit_stopped`.
pub(crate) struct SystemdManager {
    // TODO: add zbus::Connection field once the `zbus` crate dependency is
    // added. The connection is created once at startup:
    //   zbus::Connection::system().await?
    _private: (),
}

impl SystemdManager {
    /// Connect to the system D-Bus and return a `SystemdManager`.
    pub(crate) fn new() -> Self {
        Self { _private: () }
    }
}

impl ProcessManager for SystemdManager {
    type Error = SystemdError;

    async fn start_transient(&self, _spec: TransientUnitSpec) -> Result<(), Self::Error> {
        todo!("SystemdManager::start_transient")
    }

    async fn stop_unit(&self, _name: &str) -> Result<(), Self::Error> {
        todo!("SystemdManager::stop_unit")
    }

    async fn wait_unit_stopped(&self, _name: &str, _timeout: Duration) -> Result<(), Self::Error> {
        todo!("SystemdManager::wait_unit_stopped")
    }

    async fn unit_state(&self, _name: &str) -> Result<Option<UnitState>, Self::Error> {
        todo!("SystemdManager::unit_state")
    }

    async fn list_units(&self, _prefix: &str) -> Result<Vec<UnitSummary>, Self::Error> {
        todo!("SystemdManager::list_units")
    }

    async fn write_unit(&self, _name: &str, _content: &str) -> Result<(), Self::Error> {
        todo!("SystemdManager::write_unit")
    }

    async fn remove_unit(&self, _name: &str) -> Result<(), Self::Error> {
        todo!("SystemdManager::remove_unit")
    }

    async fn daemon_reload(&self) -> Result<(), Self::Error> {
        todo!("SystemdManager::daemon_reload")
    }

    async fn start_unit(&self, _name: &str) -> Result<(), Self::Error> {
        todo!("SystemdManager::start_unit")
    }
}
