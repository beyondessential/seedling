use std::{sync::Arc, time::Duration};

use snafu::Snafu;
use zbus::{
    Connection,
    zvariant::{Array, OwnedObjectPath, Signature, Value},
};

use crate::system::{
    ProcessManager,
    types::{ActiveState, TransientRestart, TransientUnitSpec, UnitState, UnitSummary},
};

const UNIT_DIR: &str = "/etc/systemd/system";

// ---------------------------------------------------------------------------
// Internal error type
// ---------------------------------------------------------------------------

#[derive(Debug, Snafu)]
pub(crate) enum SystemdError {
    #[snafu(display("D-Bus error: {source}"))]
    DBus { source: zbus::Error },
    #[snafu(display("unit {name} did not stop within the timeout"))]
    WaitTimeout { name: String },
    #[snafu(display("I/O error: {source}"))]
    Io { source: std::io::Error },
    #[snafu(display("D-Bus response has unexpected type: {message}"))]
    Protocol { message: String },
}

// ---------------------------------------------------------------------------
// D-Bus proxy — systemd Manager interface
// ---------------------------------------------------------------------------

#[zbus::proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1"
)]
trait Systemd1Manager {
    /// Start a transient unit. `properties` is `a(sv)` and `aux` is
    /// `a(sa(sv))`. Returns the D-Bus job object path.
    fn start_transient_unit(
        &self,
        name: &str,
        mode: &str,
        properties: Vec<(&str, Value<'_>)>,
        aux: Vec<(&str, Vec<(&str, Value<'_>)>)>,
    ) -> zbus::Result<OwnedObjectPath>;

    fn stop_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;

    fn start_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;

    fn get_unit(&self, name: &str) -> zbus::Result<OwnedObjectPath>;

    fn reload(&self) -> zbus::Result<()>;

    /// `ListUnits` returns `a(ssssssouso)`.
    fn list_units(
        &self,
    ) -> zbus::Result<
        Vec<(
            String,          // unit name
            String,          // description
            String,          // load state
            String,          // active state
            String,          // sub state
            String,          // following
            OwnedObjectPath, // unit object path
            u32,             // job id
            String,          // job type
            OwnedObjectPath, // job object path
        )>,
    >;
}

// ---------------------------------------------------------------------------
// D-Bus proxy — systemd Unit interface (for property reads)
// ---------------------------------------------------------------------------

#[zbus::proxy(
    interface = "org.freedesktop.systemd1.Unit",
    default_service = "org.freedesktop.systemd1"
)]
trait Systemd1Unit {
    #[zbus(property)]
    fn active_state(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn sub_state(&self) -> zbus::Result<String>;
}

// ---------------------------------------------------------------------------
// SystemdManager
// ---------------------------------------------------------------------------

pub(crate) struct SystemdManager {
    conn: Arc<Connection>,
}

impl SystemdManager {
    /// Connect to the system D-Bus. Called once at startup.
    pub(crate) async fn connect() -> Result<Self, SystemdError> {
        let conn = Connection::system()
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        Ok(Self {
            conn: Arc::new(conn),
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn restart_str(r: TransientRestart) -> &'static str {
    match r {
        TransientRestart::No => "no",
        TransientRestart::OnFailure => "on-failure",
        TransientRestart::Always => "always",
    }
}

fn parse_active_state(s: &str) -> ActiveState {
    match s {
        "active" => ActiveState::Active,
        "activating" => ActiveState::Activating,
        "deactivating" => ActiveState::Deactivating,
        "failed" => ActiveState::Failed,
        _ => ActiveState::Inactive,
    }
}

/// Builds the `a(sasb)` value required for the `ExecStart` property in
/// `StartTransientUnit`. Each outer-array element is a struct:
/// `(executable_path: s, full_argv: as, ignore_exit_code: b)`.
///
/// Construction uses `Array` + `Value::from(tuple)` — the idiomatic zvariant
/// 5.x path for building complex variant values without a serde roundtrip.
fn build_exec_start(exec_start: &[String]) -> Result<Value<'static>, SystemdError> {
    let path = exec_start.first().cloned().unwrap_or_default();

    // Inner array of strings: as
    let s_sig = Signature::try_from("s").map_err(|e| SystemdError::Protocol {
        message: format!("building 's' signature: {e}"),
    })?;
    let mut argv_arr = Array::new(&s_sig);
    for s in exec_start {
        argv_arr
            .append(Value::from(s.clone()))
            .map_err(|e| SystemdError::Protocol {
                message: format!("appending argv element: {e}"),
            })?;
    }

    // One (sasb) struct entry using Value::from on a 3-tuple.
    // zvariant implements From<(T1,T2,T3)> for Value when each Ti: Into<Value>.
    let entry = Value::from((path, Value::Array(argv_arr), false));

    // Outer a(sasb) array
    let entry_sig = Signature::try_from("(sasb)").map_err(|e| SystemdError::Protocol {
        message: format!("building '(sasb)' signature: {e}"),
    })?;
    let mut outer = Array::new(&entry_sig);
    outer.append(entry).map_err(|e| SystemdError::Protocol {
        message: format!("appending ExecStart entry: {e}"),
    })?;

    Ok(Value::Array(outer))
}

// ---------------------------------------------------------------------------
// ProcessManager impl
// ---------------------------------------------------------------------------

impl ProcessManager for SystemdManager {
    type Error = SystemdError;

    async fn start_transient(&self, spec: TransientUnitSpec) -> Result<(), Self::Error> {
        let proxy = Systemd1ManagerProxy::new(&*self.conn)
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;

        let exec_value = build_exec_start(&spec.exec_start)?;
        let restart = restart_str(spec.restart);

        let props: Vec<(&str, Value<'_>)> = vec![
            ("Description", Value::from(spec.description.as_str())),
            ("ExecStart", exec_value),
            ("Restart", Value::from(restart)),
            ("StandardOutput", Value::from("journal")),
            ("StandardError", Value::from("journal")),
        ];
        let aux: Vec<(&str, Vec<(&str, Value<'_>)>)> = vec![];

        proxy
            .start_transient_unit(&spec.name, "fail", props, aux)
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;

        Ok(())
    }

    async fn stop_unit(&self, name: &str) -> Result<(), Self::Error> {
        let proxy = Systemd1ManagerProxy::new(&*self.conn)
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        proxy
            .stop_unit(name, "replace")
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        Ok(())
    }

    async fn wait_unit_stopped(&self, name: &str, timeout: Duration) -> Result<(), Self::Error> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(SystemdError::WaitTimeout {
                    name: name.to_string(),
                });
            }
            match self.unit_state(name).await? {
                None => return Ok(()),
                Some(state) => match state.active {
                    ActiveState::Inactive | ActiveState::Failed => return Ok(()),
                    _ => {}
                },
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn unit_state(&self, name: &str) -> Result<Option<UnitState>, Self::Error> {
        let proxy = Systemd1ManagerProxy::new(&*self.conn)
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;

        let unit_path = match proxy.get_unit(name).await {
            Ok(p) => p,
            Err(e) => {
                let msg = e.to_string().to_lowercase();
                if msg.contains("no such unit") || msg.contains("not loaded") {
                    return Ok(None);
                }
                return Err(SystemdError::DBus { source: e });
            }
        };

        let unit_proxy = Systemd1UnitProxy::builder(&*self.conn)
            .destination("org.freedesktop.systemd1")
            .map_err(|e| SystemdError::DBus { source: e })?
            .path(unit_path)
            .map_err(|e| SystemdError::DBus { source: e })?
            .build()
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;

        let active = unit_proxy
            .active_state()
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        let sub = unit_proxy
            .sub_state()
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;

        Ok(Some(UnitState {
            active: parse_active_state(&active),
            sub,
        }))
    }

    async fn list_units(&self, prefix: &str) -> Result<Vec<UnitSummary>, Self::Error> {
        let proxy = Systemd1ManagerProxy::new(&*self.conn)
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;

        let raw = proxy
            .list_units()
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;

        let result = raw
            .into_iter()
            .filter(|(name, ..)| name.starts_with(prefix))
            .map(|(name, _, _, active, sub, ..)| UnitSummary {
                name,
                state: UnitState {
                    active: parse_active_state(&active),
                    sub,
                },
            })
            .collect();

        Ok(result)
    }

    async fn write_unit(&self, name: &str, content: &str) -> Result<(), Self::Error> {
        let path = std::path::Path::new(UNIT_DIR).join(name);
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| SystemdError::Io { source: e })?;
        Ok(())
    }

    async fn remove_unit(&self, name: &str) -> Result<(), Self::Error> {
        let path = std::path::Path::new(UNIT_DIR).join(name);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(SystemdError::Io { source: e }),
        }
    }

    async fn daemon_reload(&self) -> Result<(), Self::Error> {
        let proxy = Systemd1ManagerProxy::new(&*self.conn)
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        proxy
            .reload()
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        Ok(())
    }

    async fn start_unit(&self, name: &str) -> Result<(), Self::Error> {
        let proxy = Systemd1ManagerProxy::new(&*self.conn)
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        proxy
            .start_unit(name, "replace")
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        Ok(())
    }
}
