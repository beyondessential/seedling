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
    // spec: LOG#log-stream-request
    // The journal is opened on the reader thread, which owns it, so whether it
    // opened is handed back before the caller answers the request.
    let (opened_tx, opened_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);

    std::thread::Builder::new()
        .name("journal-reader".into())
        .spawn(move || {
            if let Err(e) = journal_reader_thread(opts, tx, opened_tx) {
                tracing::error!("journal reader thread failed: {e}");
            }
        })?;

    match opened_rx.recv() {
        Ok(Ok(())) => Ok(rx),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => Err("the journal reader ended before opening the journal".into()),
    }
}

/// Open the system journal, narrowed to the entries `opts` asks for and
/// positioned at the start of its tail window.
fn open_journal(
    opts: &LogStreamOptions,
) -> Result<systemd::journal::Journal, Box<dyn std::error::Error + Send + Sync>> {
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

    // spec: LOG#entry-selection
    // spec: LINUX#logs
    seek_tail_window(&mut journal, opts.tail)?;
    Ok(journal)
}

fn journal_reader_thread(
    opts: LogStreamOptions,
    tx: tokio::sync::mpsc::Sender<LogEntry>,
    opened: std::sync::mpsc::SyncSender<Result<(), String>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut journal = match open_journal(&opts) {
        Ok(journal) => {
            let _ = opened.send(Ok(()));
            journal
        }
        Err(e) => {
            let _ = opened.send(Err(e.to_string()));
            return Err(e);
        }
    };

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

/// The journal cursor moves [`seek_tail_window`] needs, so its arithmetic can
/// be tested without a running journal.
trait JournalCursor {
    fn seek_head(&mut self) -> systemd::Result<()>;
    fn seek_tail(&mut self) -> systemd::Result<()>;
    /// Step back one entry, returning whether there was one to step onto.
    fn previous(&mut self) -> systemd::Result<bool>;
}

impl JournalCursor for systemd::Journal {
    fn seek_head(&mut self) -> systemd::Result<()> {
        self.seek(systemd::journal::JournalSeek::Head)
    }

    fn seek_tail(&mut self) -> systemd::Result<()> {
        self.seek(systemd::journal::JournalSeek::Tail)
    }

    fn previous(&mut self) -> systemd::Result<bool> {
        Ok(systemd::journal::JournalRef::previous(self)? != 0)
    }
}

/// Position `journal` so that the next entry read is the oldest of its last
/// `tail` entries.
///
/// Reading advances before it reads, so the cursor goes one entry further
/// back than the window. Running out of entries first means the window is
/// the whole journal, which is then read from the head.
fn seek_tail_window(journal: &mut impl JournalCursor, tail: u64) -> systemd::Result<()> {
    journal.seek_tail()?;
    let mut stepped = 0;
    while stepped <= tail && journal.previous()? {
        stepped += 1;
    }
    if stepped <= tail {
        journal.seek_head()?;
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

    /// An in-memory journal with sd_journal's cursor semantics: a seek leaves
    /// the cursor between entries, `previous` steps onto the entry before it,
    /// and reading steps onto the entry after it before reading.
    struct FakeJournal {
        len: usize,
        /// Index of the entry the cursor is on, or the gap before `0` when
        /// `None`; a tail seek sits past the last entry at `Some(len)`.
        at: Option<usize>,
    }

    impl FakeJournal {
        fn new(len: usize) -> Self {
            Self { len, at: None }
        }

        fn read_all(&mut self) -> Vec<usize> {
            let mut read = Vec::new();
            loop {
                let next = self.at.map_or(0, |i| i + 1);
                if next >= self.len {
                    return read;
                }
                self.at = Some(next);
                read.push(next);
            }
        }
    }

    impl JournalCursor for FakeJournal {
        fn seek_head(&mut self) -> systemd::Result<()> {
            self.at = None;
            Ok(())
        }

        fn seek_tail(&mut self) -> systemd::Result<()> {
            self.at = Some(self.len);
            Ok(())
        }

        fn previous(&mut self) -> systemd::Result<bool> {
            match self.at {
                Some(i) if i > 0 => {
                    self.at = Some(i - 1);
                    Ok(true)
                }
                _ => Ok(false),
            }
        }
    }

    fn fake_window(len: usize, tail: u64) -> Vec<usize> {
        let mut journal = FakeJournal::new(len);
        seek_tail_window(&mut journal, tail).unwrap();
        journal.read_all()
    }

    // spec: LOG#entry-selection
    #[test]
    fn tail_window_arithmetic() {
        assert!(fake_window(3, 0).is_empty());
        assert_eq!(fake_window(3, 1), [2]);
        assert_eq!(fake_window(3, 2), [1, 2]);
        assert_eq!(fake_window(3, 3), [0, 1, 2]);
        assert_eq!(fake_window(3, 10), [0, 1, 2]);
        assert!(fake_window(0, 5).is_empty());
    }

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

    // spec: LOG#entry-selection
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
