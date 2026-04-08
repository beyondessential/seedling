use std::{sync::Arc, time::Duration};

use snafu::Snafu;
use zbus::{
    Connection,
    zvariant::{Array, OwnedObjectPath, Signature, StructureBuilder, Value},
};

use crate::system::{
    BoxError, BoxFuture, ProcessManager,
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

#[derive(serde::Serialize, zbus::zvariant::Type)]
struct UnitProperty<'a> {
    name: &'a str,
    value: Value<'a>,
}

#[derive(serde::Serialize, zbus::zvariant::Type)]
struct AuxUnit<'a> {
    name: &'a str,
    properties: Vec<UnitProperty<'a>>,
}

#[derive(Debug, serde::Deserialize, zbus::zvariant::Type)]
struct ListedUnit {
    name: String,

    // unused here, still needs to be present to deserialize correctly
    #[expect(dead_code, reason = "unused here")]
    description: String,
    #[expect(dead_code, reason = "unused here")]
    load_state: String,

    active_state: String,
    sub_state: String,

    #[expect(dead_code, reason = "unused here")]
    following: String,
    #[expect(dead_code, reason = "unused here")]
    unit_path: OwnedObjectPath,
    #[expect(dead_code, reason = "unused here")]
    job_id: u32,
    #[expect(dead_code, reason = "unused here")]
    job_type: String,
    #[expect(dead_code, reason = "unused here")]
    job_path: OwnedObjectPath,
}

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
        properties: Vec<UnitProperty<'_>>,
        aux: Vec<AuxUnit<'_>>,
    ) -> zbus::Result<OwnedObjectPath>;

    fn stop_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;

    fn reset_failed_unit(&self, name: &str) -> zbus::Result<()>;

    fn start_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;

    fn get_unit(&self, name: &str) -> zbus::Result<OwnedObjectPath>;

    fn reload(&self) -> zbus::Result<()>;

    /// `ListUnits` returns `a(ssssssouso)`.
    fn list_units(&self) -> zbus::Result<Vec<ListedUnit>>;
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

    #[tracing::instrument(skip_all, fields(unit = %spec.name))]
    async fn start_transient_impl(&self, spec: TransientUnitSpec) -> Result<(), SystemdError> {
        let proxy = Systemd1ManagerProxy::new(&self.conn)
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;

        let exec_value = build_exec_start(&spec.exec_start)?;
        let restart = restart_str(spec.restart);

        let props = vec![
            UnitProperty {
                name: "Description",
                value: Value::from(spec.description.as_str()),
            },
            UnitProperty {
                name: "ExecStart",
                value: exec_value,
            },
            UnitProperty {
                name: "Restart",
                value: Value::from(restart),
            },
            UnitProperty {
                name: "StandardOutput",
                value: Value::from("journal"),
            },
            UnitProperty {
                name: "StandardError",
                value: Value::from("journal"),
            },
        ];
        let aux: Vec<AuxUnit<'_>> = vec![];

        proxy
            .start_transient_unit(&spec.name, "fail", props, aux)
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;

        Ok(())
    }

    #[tracing::instrument(skip_all, fields(%name))]
    async fn stop_unit_impl(&self, name: &str) -> Result<(), SystemdError> {
        let proxy = Systemd1ManagerProxy::new(&self.conn)
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        proxy
            .stop_unit(name, "replace")
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        Ok(())
    }

    async fn reset_failed_unit_impl(&self, name: &str) -> Result<(), SystemdError> {
        let proxy = Systemd1ManagerProxy::new(&self.conn)
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        proxy
            .reset_failed_unit(name)
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        Ok(())
    }

    async fn wait_unit_stopped_impl(
        &self,
        name: &str,
        timeout: Duration,
    ) -> Result<(), SystemdError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(SystemdError::WaitTimeout {
                    name: name.to_string(),
                });
            }
            match self.unit_state_impl(name).await? {
                None => return Ok(()),
                Some(state) => match state.active {
                    ActiveState::Inactive | ActiveState::Failed => return Ok(()),
                    _ => {}
                },
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn unit_state_impl(&self, name: &str) -> Result<Option<UnitState>, SystemdError> {
        let proxy = Systemd1ManagerProxy::new(&self.conn)
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

        let unit_proxy = Systemd1UnitProxy::builder(&self.conn)
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

    async fn list_units_impl(&self, prefix: &str) -> Result<Vec<UnitSummary>, SystemdError> {
        let proxy = Systemd1ManagerProxy::new(&self.conn)
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;

        let raw = proxy
            .list_units()
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;

        let result = raw
            .into_iter()
            .filter(|u| u.name.starts_with(prefix))
            .map(|u| UnitSummary {
                name: u.name,
                state: UnitState {
                    active: parse_active_state(&u.active_state),
                    sub: u.sub_state,
                },
            })
            .collect();

        Ok(result)
    }

    async fn write_unit_impl(&self, name: &str, content: &str) -> Result<(), SystemdError> {
        let path = std::path::Path::new(UNIT_DIR).join(name);
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| SystemdError::Io { source: e })?;
        Ok(())
    }

    async fn remove_unit_impl(&self, name: &str) -> Result<(), SystemdError> {
        let path = std::path::Path::new(UNIT_DIR).join(name);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(SystemdError::Io { source: e }),
        }
    }

    async fn daemon_reload_impl(&self) -> Result<(), SystemdError> {
        let proxy = Systemd1ManagerProxy::new(&self.conn)
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        proxy
            .reload()
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        Ok(())
    }

    async fn start_unit_impl(&self, name: &str) -> Result<(), SystemdError> {
        let proxy = Systemd1ManagerProxy::new(&self.conn)
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        proxy
            .start_unit(name, "replace")
            .await
            .map_err(|e| SystemdError::DBus { source: e })?;
        Ok(())
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

    // Build one (sasb) struct entry. We must use append_field (which pushes the
    // Value directly) rather than add_field (which routes through Value::new and
    // wraps any Value<'_> as a variant because Value::SIGNATURE == "v").
    let entry = Value::Structure(
        StructureBuilder::new()
            .append_field(Value::from(path))
            .append_field(Value::Array(argv_arr))
            .append_field(Value::from(false))
            .build()
            .map_err(|e| SystemdError::Protocol {
                message: format!("building ExecStart entry: {e}"),
            })?,
    );

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
    fn start_transient<'a>(
        &'a self,
        spec: TransientUnitSpec,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.start_transient_impl(spec).await.map_err(Into::into) })
    }

    fn stop_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.stop_unit_impl(name).await.map_err(Into::into) })
    }

    fn reset_failed_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.reset_failed_unit_impl(name).await.map_err(Into::into) })
    }

    fn wait_unit_stopped<'a>(
        &'a self,
        name: &'a str,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move {
            self.wait_unit_stopped_impl(name, timeout)
                .await
                .map_err(Into::into)
        })
    }

    fn unit_state<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Option<UnitState>, BoxError>> {
        Box::pin(async move { self.unit_state_impl(name).await.map_err(Into::into) })
    }

    fn list_units<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<UnitSummary>, BoxError>> {
        Box::pin(async move { self.list_units_impl(prefix).await.map_err(Into::into) })
    }

    fn write_unit<'a>(
        &'a self,
        name: &'a str,
        content: &'a str,
    ) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move {
            self.write_unit_impl(name, content)
                .await
                .map_err(Into::into)
        })
    }

    fn remove_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.remove_unit_impl(name).await.map_err(Into::into) })
    }

    fn daemon_reload<'a>(&'a self) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.daemon_reload_impl().await.map_err(Into::into) })
    }

    fn start_unit<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<(), BoxError>> {
        Box::pin(async move { self.start_unit_impl(name).await.map_err(Into::into) })
    }
}
