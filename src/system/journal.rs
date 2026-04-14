use std::collections::BTreeMap;

use serde::Serialize;

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
}

/// Options for the log stream.
#[derive(Debug, Clone)]
pub struct LogStreamOptions {
    pub target: LogTarget,
    pub follow: bool,
    pub tail: u64,
}

/// Spawn a journal reader on a dedicated OS thread and return a channel of
/// log entries.
///
/// The reader applies journal field matches based on the target, seeks to the
/// appropriate tail position, reads historical entries, then optionally follows
/// for new entries.
///
/// The returned receiver yields entries until the journal is exhausted (in
/// non-follow mode) or the receiver is dropped (which causes the reader
/// thread to exit on the next send attempt).
pub fn spawn_log_reader(
    opts: LogStreamOptions,
) -> Result<tokio::sync::mpsc::Receiver<LogEntry>, Box<dyn std::error::Error + Send + Sync>> {
    let (tx, rx) = tokio::sync::mpsc::channel(256);

    std::thread::Builder::new()
        .name("journal-reader".into())
        .spawn(move || {
            if let Err(e) = journal_reader_thread(opts, tx) {
                tracing::error!("journal reader thread failed: {e}");
            }
        })?;

    Ok(rx)
}

fn journal_reader_thread(
    opts: LogStreamOptions,
    tx: tokio::sync::mpsc::Sender<LogEntry>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use systemd::journal::{JournalSeek, OpenOptions};

    let mut journal = OpenOptions::default().system(true).open()?;

    match &opts.target {
        LogTarget::App {
            app,
            resource,
            instance,
        } => {
            journal.match_add("SEEDLING_APP", app.as_bytes())?;
            if let Some(res) = resource {
                journal.match_add("SEEDLING_RESOURCE", res.as_bytes())?;
            }
            if let Some(inst) = instance {
                journal.match_add("SEEDLING_INSTANCE", inst.as_bytes())?;
            }
        }
        LogTarget::Infra(component) => {
            let value = match component {
                InfraComponent::Proxy => "proxy",
                InfraComponent::Resolver => "resolver",
            };
            journal.match_add("SEEDLING_INFRA", value)?;
        }
    }

    // Seek to the tail, then walk backwards to find the starting position.
    journal.seek(JournalSeek::Tail)?;
    for _ in 0..opts.tail {
        if journal.previous()? == 0 {
            break;
        }
    }

    loop {
        match journal.next_entry()? {
            Some(entry) => {
                let log_entry = record_to_entry(&entry);
                if tx.blocking_send(log_entry).is_err() {
                    break;
                }
            }
            None => {
                if !opts.follow {
                    break;
                }
                journal.wait(Some(std::time::Duration::from_secs(1)))?;
            }
        }
    }

    Ok(())
}

fn record_to_entry(fields: &BTreeMap<String, String>) -> LogEntry {
    let timestamp = fields
        .get("__REALTIME_TIMESTAMP")
        .and_then(|us_str| us_str.parse::<i64>().ok())
        .and_then(|us| jiff::Timestamp::from_microsecond(us).ok())
        .map(|ts| ts.to_string())
        .unwrap_or_default();

    let message = fields.get("MESSAGE").cloned().unwrap_or_default();

    let unit = fields.get("_SYSTEMD_UNIT").cloned().unwrap_or_default();

    // Infer stdout vs stderr from syslog priority.
    // Priority <= 3 (error and below) maps to stderr, everything else to stdout.
    let stream = fields
        .get("PRIORITY")
        .and_then(|p| p.parse::<u8>().ok())
        .map(|p| if p <= 3 { "stderr" } else { "stdout" })
        .unwrap_or("stdout")
        .to_owned();

    LogEntry {
        timestamp,
        message,
        unit,
        stream,
        app: fields.get("SEEDLING_APP").cloned(),
        resource_kind: fields.get("SEEDLING_RESOURCE_KIND").cloned(),
        resource: fields.get("SEEDLING_RESOURCE").cloned(),
        instance: fields.get("SEEDLING_INSTANCE").cloned(),
        infra: fields.get("SEEDLING_INFRA").cloned(),
    }
}
