use std::sync::Arc;

use snafu::{IntoError, ResultExt, Snafu};
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
    DBus {
        source: zbus::Error,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("I/O error: {source}"))]
    Io {
        source: std::io::Error,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("D-Bus response has unexpected type: {message}"))]
    Protocol {
        message: String,
        backtrace: snafu::Backtrace,
    },
    #[snafu(display("invalid unit name: {name}"))]
    InvalidUnitName {
        name: String,
        backtrace: snafu::Backtrace,
    },
}

// ---------------------------------------------------------------------------
// Unit name validation
// ---------------------------------------------------------------------------

/// Reject unit names that could cause path traversal when joined with
/// `UNIT_DIR`, or that are not valid systemd unit names.
// TODO: use https://docs.rs/systemd/latest/systemd/unit/fn.escape_name.html
fn validate_unit_name(name: &str) -> Result<(), SystemdError> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name == "."
        || name == ".."
    {
        return Err(InvalidUnitNameSnafu {
            name: name.to_owned(),
        }
        .build());
    }

    const SUFFIXES: &[&str] = &[
        ".service",
        ".socket",
        ".target",
        ".mount",
        ".automount",
        ".swap",
        ".timer",
        ".path",
        ".slice",
        ".scope",
    ];
    if !SUFFIXES.iter().any(|s| name.ends_with(s)) {
        return Err(InvalidUnitNameSnafu {
            name: name.to_owned(),
        }
        .build());
    }

    Ok(())
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
        let conn = Connection::system().await.context(DBusSnafu)?;
        Ok(Self {
            conn: Arc::new(conn),
        })
    }

    #[tracing::instrument(skip_all, fields(unit = %spec.name))]
    async fn start_transient_impl(&self, spec: TransientUnitSpec) -> Result<(), SystemdError> {
        validate_unit_name(&spec.name)?;
        let proxy = Systemd1ManagerProxy::new(&self.conn)
            .await
            .context(DBusSnafu)?;

        let exec_value = build_exec_start(&spec.exec_start)?;
        let restart = restart_str(spec.restart);

        let mut props = vec![
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
            // Garbage-collect the transient unit (and remove its file from
            // /run/systemd/transient/) as soon as it reaches inactive or
            // failed state with no pending restart jobs. Without this the
            // default CollectMode=inactive leaves units stuck in
            // failed/start-limit-hit loaded indefinitely.
            UnitProperty {
                name: "CollectMode",
                value: Value::from("inactive-or-failed"),
            },
        ];
        // r[impl actuate.container.journal-metadata]
        // r[impl actuate.infra.journal-metadata]
        if !spec.log_extra_fields.is_empty() {
            let ay_sig = Signature::try_from("ay").map_err(|e| {
                ProtocolSnafu {
                    message: format!("building 'ay' signature: {e}"),
                }
                .build()
            })?;
            let mut outer = Array::new(&ay_sig);
            for (key, val) in &spec.log_extra_fields {
                let field = format!("{key}={val}");
                let y_sig = Signature::try_from("y").map_err(|e| {
                    ProtocolSnafu {
                        message: format!("building 'y' signature: {e}"),
                    }
                    .build()
                })?;
                let mut bytes = Array::new(&y_sig);
                for &b in field.as_bytes() {
                    bytes.append(Value::U8(b)).map_err(|e| {
                        ProtocolSnafu {
                            message: format!("appending LogExtraFields byte: {e}"),
                        }
                        .build()
                    })?;
                }
                outer.append(Value::Array(bytes)).map_err(|e| {
                    ProtocolSnafu {
                        message: format!("appending LogExtraFields entry: {e}"),
                    }
                    .build()
                })?;
            }
            props.push(UnitProperty {
                name: "LogExtraFields",
                value: Value::Array(outer),
            });
        }

        let aux: Vec<AuxUnit<'_>> = vec![];

        proxy
            .start_transient_unit(&spec.name, "fail", props, aux)
            .await
            .context(DBusSnafu)?;

        Ok(())
    }

    #[tracing::instrument(skip_all, fields(%name))]
    async fn stop_unit_impl(&self, name: &str) -> Result<(), SystemdError> {
        validate_unit_name(name)?;
        let proxy = Systemd1ManagerProxy::new(&self.conn)
            .await
            .context(DBusSnafu)?;
        match proxy.stop_unit(name, "replace").await {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string().to_lowercase();
                if msg.contains("no such unit") || msg.contains("not loaded") {
                    Ok(())
                } else {
                    Err(DBusSnafu.into_error(e))
                }
            }
        }
    }

    async fn reset_failed_unit_impl(&self, name: &str) -> Result<(), SystemdError> {
        validate_unit_name(name)?;
        let proxy = Systemd1ManagerProxy::new(&self.conn)
            .await
            .context(DBusSnafu)?;
        match proxy.reset_failed_unit(name).await {
            Ok(()) => Ok(()),
            Err(e) => {
                let msg = e.to_string().to_lowercase();
                if msg.contains("no such unit") || msg.contains("not loaded") {
                    Ok(())
                } else {
                    Err(DBusSnafu.into_error(e))
                }
            }
        }
    }

    async fn unit_state_impl(&self, name: &str) -> Result<Option<UnitState>, SystemdError> {
        validate_unit_name(name)?;
        let proxy = Systemd1ManagerProxy::new(&self.conn)
            .await
            .context(DBusSnafu)?;

        let unit_path = match proxy.get_unit(name).await {
            Ok(p) => p,
            Err(e) => {
                let msg = e.to_string().to_lowercase();
                if msg.contains("no such unit") || msg.contains("not loaded") {
                    return Ok(None);
                }
                return Err(DBusSnafu.into_error(e));
            }
        };

        let unit_proxy = Systemd1UnitProxy::builder(&self.conn)
            .destination("org.freedesktop.systemd1")
            .context(DBusSnafu)?
            .path(unit_path)
            .context(DBusSnafu)?
            .build()
            .await
            .context(DBusSnafu)?;

        let active = unit_proxy.active_state().await.context(DBusSnafu)?;
        let sub = unit_proxy.sub_state().await.context(DBusSnafu)?;

        Ok(Some(UnitState {
            active: parse_active_state(&active),
            sub,
        }))
    }

    async fn list_units_impl(&self, prefix: &str) -> Result<Vec<UnitSummary>, SystemdError> {
        let proxy = Systemd1ManagerProxy::new(&self.conn)
            .await
            .context(DBusSnafu)?;

        let raw = proxy.list_units().await.context(DBusSnafu)?;

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
        validate_unit_name(name)?;
        super::confined_write::write_async(
            std::path::Path::new(UNIT_DIR),
            name,
            content.as_bytes(),
        )
        .await
        .map_err(|e| match e {
            super::confined_write::ConfinedWriteError::Io { source, .. } => {
                IoSnafu.into_error(source)
            }
            super::confined_write::ConfinedWriteError::Escape { .. } => InvalidUnitNameSnafu {
                name: name.to_owned(),
            }
            .build(),
        })?;
        Ok(())
    }

    async fn remove_unit_impl(&self, name: &str) -> Result<(), SystemdError> {
        validate_unit_name(name)?;
        let path = std::path::Path::new(UNIT_DIR).join(name);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(IoSnafu.into_error(e)),
        }
    }

    async fn daemon_reload_impl(&self) -> Result<(), SystemdError> {
        let proxy = Systemd1ManagerProxy::new(&self.conn)
            .await
            .context(DBusSnafu)?;
        proxy.reload().await.context(DBusSnafu)?;
        Ok(())
    }

    async fn start_unit_impl(&self, name: &str) -> Result<(), SystemdError> {
        validate_unit_name(name)?;
        let proxy = Systemd1ManagerProxy::new(&self.conn)
            .await
            .context(DBusSnafu)?;
        proxy.start_unit(name, "replace").await.context(DBusSnafu)?;
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
    let s_sig = Signature::try_from("s").map_err(|e| {
        ProtocolSnafu {
            message: format!("building 's' signature: {e}"),
        }
        .build()
    })?;
    let mut argv_arr = Array::new(&s_sig);
    for s in exec_start {
        argv_arr.append(Value::from(s.clone())).map_err(|e| {
            ProtocolSnafu {
                message: format!("appending argv element: {e}"),
            }
            .build()
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
            .map_err(|e| {
                ProtocolSnafu {
                    message: format!("building ExecStart entry: {e}"),
                }
                .build()
            })?,
    );

    // Outer a(sasb) array
    let entry_sig = Signature::try_from("(sasb)").map_err(|e| {
        ProtocolSnafu {
            message: format!("building '(sasb)' signature: {e}"),
        }
        .build()
    })?;
    let mut outer = Array::new(&entry_sig);
    outer.append(entry).map_err(|e| {
        ProtocolSnafu {
            message: format!("appending ExecStart entry: {e}"),
        }
        .build()
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

#[cfg(test)]
mod tests {
    use super::validate_unit_name;

    #[test]
    fn accepts_valid_service() {
        validate_unit_name("seedling-myapp-web.service").unwrap();
    }

    #[test]
    fn accepts_valid_timer() {
        validate_unit_name("backup.timer").unwrap();
    }

    #[test]
    fn accepts_valid_socket() {
        validate_unit_name("my-app.socket").unwrap();
    }

    #[test]
    fn accepts_valid_scope() {
        validate_unit_name("session-42.scope").unwrap();
    }

    #[test]
    fn rejects_empty() {
        validate_unit_name("").unwrap_err();
    }

    #[test]
    fn rejects_slash() {
        validate_unit_name("../etc/cron.d/evil.service").unwrap_err();
    }

    #[test]
    fn rejects_backslash() {
        validate_unit_name("foo\\bar.service").unwrap_err();
    }

    #[test]
    fn rejects_null_byte() {
        validate_unit_name("foo\0bar.service").unwrap_err();
    }

    #[test]
    fn rejects_dot() {
        validate_unit_name(".").unwrap_err();
    }

    #[test]
    fn rejects_dotdot() {
        validate_unit_name("..").unwrap_err();
    }

    #[test]
    fn rejects_no_suffix() {
        validate_unit_name("seedling-web").unwrap_err();
    }

    #[test]
    fn rejects_path_traversal_relative() {
        validate_unit_name("../../tmp/evil.service").unwrap_err();
    }

    #[test]
    fn rejects_absolute_path() {
        validate_unit_name("/etc/systemd/system/evil.service").unwrap_err();
    }
}
