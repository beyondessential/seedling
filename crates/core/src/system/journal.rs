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
    use systemd::journal::OpenOptions;

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

    // i[impl logs.tail]
    seek_tail_window(&mut journal, opts.tail)?;

    loop {
        match journal.next_entry()? {
            Some(entry) => {
                // timestamp_usec() reads from the entry the cursor is currently
                // positioned on. next_entry() leaves the cursor on the entry it
                // just returned, so this call refers to the correct entry.
                let timestamp_us = journal.timestamp_usec().unwrap_or(0);
                let log_entry = record_to_entry(timestamp_us, &entry);
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

/// Position `journal` so that the next entry read is the oldest of its last
/// `tail` entries.
///
/// Reading advances before it reads, so the cursor goes one entry further
/// back than the window. Running out of entries first means the window is
/// the whole journal, which is then read from the head.
fn seek_tail_window(journal: &mut systemd::Journal, tail: u64) -> systemd::Result<()> {
    use systemd::journal::JournalSeek;

    journal.seek(JournalSeek::Tail)?;
    let mut stepped = 0;
    while stepped <= tail && journal.previous()? != 0 {
        stepped += 1;
    }
    if stepped <= tail {
        journal.seek(JournalSeek::Head)?;
    }
    Ok(())
}

fn record_to_entry(timestamp_us: u64, fields: &BTreeMap<String, String>) -> LogEntry {
    let timestamp = jiff::Timestamp::from_microsecond(timestamp_us as i64)
        .ok()
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
        rt_call: fields.get("SEEDLING_RT_CALL").cloned(),
        script_pos: fields.get("SEEDLING_SCRIPT_POS").cloned(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use systemd::journal::OpenOptions;

    use super::*;

    const MARKER_FIELD: &str = "SEEDLING_TEST_MARKER";

    fn open_marked(marker: &str) -> systemd::Journal {
        let mut journal = OpenOptions::default()
            .current_user(true)
            .open()
            .expect("open journal");
        journal.match_add(MARKER_FIELD, marker).expect("match");
        journal
    }

    fn count_marked(marker: &str) -> usize {
        let mut journal = open_marked(marker);
        let mut count = 0;
        while journal.next().expect("read") != 0 {
            count += 1;
        }
        count
    }

    fn read_window(marker: &str, tail: u64) -> Vec<String> {
        let mut journal = open_marked(marker);
        seek_tail_window(&mut journal, tail).expect("seek");
        let mut messages = Vec::new();
        while let Some(entry) = journal.next_entry().expect("read") {
            messages.push(entry["MESSAGE"].clone());
        }
        messages
    }

    // i[verify logs.tail]
    #[test]
    #[ignore = "needs a running journald"]
    fn tail_window_holds_the_last_entries() {
        let marker = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        for i in 0..3 {
            systemd::journal::send(&[
                &format!("MESSAGE=entry {i}"),
                &format!("{MARKER_FIELD}={marker}"),
            ]);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while count_marked(&marker) < 3 {
            assert!(
                Instant::now() < deadline,
                "entries never reached the journal"
            );
            std::thread::sleep(Duration::from_millis(100));
        }

        assert!(read_window(&marker, 0).is_empty());
        assert_eq!(read_window(&marker, 1), ["entry 2"]);
        assert_eq!(read_window(&marker, 2), ["entry 1", "entry 2"]);
        assert_eq!(read_window(&marker, 3), ["entry 0", "entry 1", "entry 2"]);
        assert_eq!(read_window(&marker, 10), ["entry 0", "entry 1", "entry 2"]);
    }
}
