//! What the runtime writes to the host's system journal and reads back from
//! it. The platform provides the journal itself: a [`JournalSink`] installed
//! once at startup, and a [`JournalSource`] held by the
//! [`System`](crate::system::System).

use std::sync::{Arc, OnceLock};

use serde::Serialize;

use crate::{runtime::identity::ResourceInstance, system::BoxError};

// spec: ACTU#container-logs
pub const APP_FIELD: &str = "SEEDLING_APP";
// spec: ACTU#container-logs
pub const RESOURCE_KIND_FIELD: &str = "SEEDLING_RESOURCE_KIND";
// spec: ACTU#container-logs
pub const RESOURCE_FIELD: &str = "SEEDLING_RESOURCE";
// spec: ACTU#container-logs
pub const INSTANCE_FIELD: &str = "SEEDLING_INSTANCE";
// spec: ACTU#container-logs
pub const INFRA_FIELD: &str = "SEEDLING_INFRA";
// spec: ACTU#container-logs
pub const RT_CALL_FIELD: &str = "SEEDLING_RT_CALL";
// spec: ACTU#container-logs
pub const SCRIPT_POS_FIELD: &str = "SEEDLING_SCRIPT_POS";

/// The journal fields that tie an entry to a workload instance, which is how
/// its logs are found. The unit of the instance's container carries them, and
/// so does anything else recorded with that container's logs.
// spec: LINUX#logs
pub fn instance_log_fields(instance: &ResourceInstance) -> Vec<(String, String)> {
    let kind = instance.kind.type_name();
    let resource = instance.name.as_deref().unwrap_or(&instance.display_name);
    vec![
        (APP_FIELD.to_owned(), instance.app.as_str().to_owned()),
        (RESOURCE_KIND_FIELD.to_owned(), kind.to_owned()),
        (RESOURCE_FIELD.to_owned(), resource.to_owned()),
        (INSTANCE_FIELD.to_owned(), instance.display_name.clone()),
    ]
}

/// How severe a journal entry is. A reader tells standard error from standard
/// output by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Error,
}

/// One entry the runtime writes to the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRecord {
    pub level: LogLevel,
    pub message: String,
    /// The `SEEDLING_*` fields the entry carries, as `(KEY, VALUE)`.
    pub fields: Vec<(String, String)>,
}

/// Writes entries to the host's system journal.
pub trait JournalSink: Send + Sync + 'static {
    fn send(&self, record: &JournalRecord) -> Result<(), BoxError>;
}

/// Reads log streams back from the host's system journal.
pub trait JournalSource: Send + Sync + 'static {
    /// Open a stream of the entries `opts` selects.
    ///
    /// The journal is open by the time this returns, so a journal that cannot
    /// be opened is an error here rather than an empty stream. The receiver
    /// yields entries until the journal is exhausted (in non-follow mode) or
    /// the receiver is dropped.
    // spec: LOG#log-stream-request
    fn open(
        &self,
        opts: LogStreamOptions,
    ) -> Result<tokio::sync::mpsc::Receiver<LogEntry>, BoxError>;
}

static SINK: OnceLock<Arc<dyn JournalSink>> = OnceLock::new();

/// Install the journal the runtime writes to. First call wins; later calls
/// are ignored.
pub fn install_sink(sink: Arc<dyn JournalSink>) {
    let _ = SINK.set(sink);
}

/// Write `record` to the installed journal.
///
/// `Ok(false)` when no journal is installed, as outside the daemon, where
/// there is nowhere to write it.
pub(crate) fn send(record: &JournalRecord) -> Result<bool, BoxError> {
    match SINK.get() {
        Some(sink) => sink.send(record).map(|()| true),
        None => Ok(false),
    }
}

/// The journal entries that record what a command run in `instance`'s
/// container wrote: one per line, on the stream it was written to.
///
/// Stderr is recorded at error level, which is how a reader tells the two
/// apart.
// spec: RT#command-execution
pub fn exec_output_entries(
    instance: &ResourceInstance,
    stdout: &[u8],
    stderr: &[u8],
) -> Vec<JournalRecord> {
    let fields = instance_log_fields(instance);
    let mut entries = Vec::new();
    for (level, bytes) in [(LogLevel::Info, stdout), (LogLevel::Error, stderr)] {
        for line in String::from_utf8_lossy(bytes).lines() {
            entries.push(JournalRecord {
                level,
                message: line.to_owned(),
                fields: fields.clone(),
            });
        }
    }
    entries
}

/// Record what a command run in `instance`'s container wrote with that
/// container's logs.
// spec: RT#command-execution
pub(crate) fn record_exec_output(instance: &ResourceInstance, stdout: &[u8], stderr: &[u8]) {
    let mut failed = 0_usize;
    let mut first_error = None;
    for entry in exec_output_entries(instance, stdout, stderr) {
        if let Err(e) = send(&entry) {
            failed += 1;
            first_error.get_or_insert(e);
        }
    }
    if let Some(e) = first_error {
        tracing::error!(
            app = %instance.app,
            instance = %instance.display_name,
            failed,
            "command output could not be recorded with the container's logs: {e}"
        );
    }
}

/// Specifies which journal entries to read.
#[derive(Debug, Clone)]
pub enum LogTarget {
    /// Workload container logs, filtered by app and optionally resource/instance.
    App {
        app: String,
        resource: Option<String>,
        instance: Option<String>,
    },
    /// Infrastructure component logs.
    Infra(InfraComponent),
}

#[derive(Debug, Clone, Copy)]
pub enum InfraComponent {
    Proxy,
    Resolver,
}

/// A single journal log entry, ready for JSON serialisation.
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub message: String,
    pub unit: String,
    pub stream: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub infra: Option<String>,
    /// rt.* primitive name, set when the entry is a Seedling-side
    /// breadcrumb rather than container output. See
    /// `system::breadcrumb` for the full set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rt_call: Option<String>,
    /// Script position (`<line>:<col>`) of the rt.* call site, set
    /// when Rhai surfaced one to the breadcrumb emitter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_pos: Option<String>,
}

/// Options for the log stream.
#[derive(Debug, Clone)]
pub struct LogStreamOptions {
    pub target: LogTarget,
    pub follow: bool,
    pub tail: u64,
}

#[cfg(test)]
mod exec_output_tests {
    use super::*;
    use crate::defs::resource::ResourceKind;

    // spec: RT#command-execution
    #[test]
    fn exec_output_is_recorded_under_the_containers_own_fields_by_line_and_stream() {
        let instance = ResourceInstance::new_singleton(
            seedling_protocol::names::AppName::new("demo").unwrap(),
            ResourceKind::Deployment,
            "api",
        );
        let entries = exec_output_entries(&instance, b"one\ntwo\n", b"oops\n");
        assert_eq!(entries.len(), 3);
        for entry in &entries {
            assert_eq!(entry.fields, instance_log_fields(&instance));
        }
        assert_eq!(
            (entries[0].message.as_str(), entries[0].level),
            ("one", LogLevel::Info)
        );
        assert_eq!(
            (entries[2].message.as_str(), entries[2].level),
            ("oops", LogLevel::Error)
        );
    }
}
