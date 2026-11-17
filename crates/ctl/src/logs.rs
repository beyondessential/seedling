use std::io::{self, Write};

use seedling_protocol::client::OiClient;

/// Run a log streaming session against an already-connected client.
///
/// `params` is the JSON object sent as the `params` field of the
/// `/logs/stream` request. `json_mode` controls whether entries are
/// printed as raw JSON lines or as human-readable text. `follow`
/// determines whether we listen for Ctrl-C (follow mode) or simply
/// drain the stream until the server closes it.
// spec: CLI#log-display
pub(super) async fn stream_logs(
    client: &OiClient,
    params: serde_json::Value,
    json_mode: bool,
    follow: bool,
) {
    match run_log_session(client, params, json_mode, follow).await {
        Ok(()) => {}
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

async fn run_log_session(
    client: &OiClient,
    params: serde_json::Value,
    json_mode: bool,
    follow: bool,
) -> Result<(), String> {
    let mut log_stream = client
        .open_subscription("/logs/stream", params)
        .await
        .map_err(|e| e.to_string())?;

    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let stdout = io::stdout();
    let mut out = stdout.lock();

    // spec: CLI#shutdown
    loop {
        let result = if follow {
            tokio::select! {
                r = log_stream.read(&mut tmp) => r,
                _ = crate::signals::shutdown_requested() => return Ok(()),
            }
        } else {
            log_stream.read(&mut tmp).await
        };

        match result {
            Ok(Some(n)) => {
                buf.extend_from_slice(&tmp[..n]);
                while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                    let line = &buf[..pos];
                    if !line.is_empty() {
                        match print_entry(&mut out, line, json_mode) {
                            Ok(()) => {}
                            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
                            Err(e) => return Err(format!("write stdout: {e}")),
                        }
                    }
                    buf.drain(..=pos);
                }
            }
            Ok(None) => break,
            Err(e) => return Err(format!("log stream: {e}")),
        }
    }

    // Flush any remaining partial line.
    if !buf.is_empty() {
        match print_entry(&mut out, &buf, json_mode) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            Err(e) => return Err(format!("write stdout: {e}")),
        }
    }

    Ok(())
}

fn print_entry(out: &mut impl Write, line: &[u8], json_mode: bool) -> io::Result<()> {
    if json_mode {
        return writeln!(out, "{}", String::from_utf8_lossy(line));
    }
    writeln!(out, "{}", format_entry(line))
}

/// An entry as one line of text: its timestamp, its most specific source name
/// and its message. A line that is not a JSON object is returned as received.
// spec: CLI#log-display
fn format_entry(line: &[u8]) -> String {
    let received = || String::from_utf8_lossy(line).into_owned();
    let Ok(serde_json::Value::Object(entry)) = serde_json::from_slice(line) else {
        return received();
    };
    let text = |key: &str| entry.get(key).and_then(|s| s.as_str());

    let timestamp = text("timestamp").unwrap_or("");
    let message = text("message").unwrap_or("");

    // The first of the instance, resource, app and infrastructure component
    // the entry carries.
    let display = ["instance", "resource", "app", "infra"]
        .into_iter()
        .find_map(text)
        .unwrap_or("");

    // Mark Seedling-side breadcrumbs ("rt.start(...)", "Action.invoke(...)",
    // unit-create, replay markers) with a `>` so the operator can tell them
    // from container stdout/stderr at a glance.
    // Script positions (when present) trail the message so the operator
    // can jump straight to the line in the BSL script.
    let marker = if text("rt_call").is_some() { "> " } else { "" };
    let pos_suffix = text("script_pos")
        .map(|p| format!("  ({})", one_line(p)))
        .unwrap_or_default();
    format!(
        "{} {} {marker}{}{pos_suffix}",
        one_line(timestamp),
        one_line(display),
        one_line(message)
    )
}

/// `text` with every control character other than a tab written as an escape,
/// so that an entry stays on one line and cannot drive the terminal.
fn one_line(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control() && c != '\t' {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::format_entry;

    // spec: CLI#log-display
    #[test]
    fn an_entry_is_one_line_with_the_most_specific_source_name() {
        let entry = br#"{"timestamp":"2026-01-01T00:00:00.000001Z","message":"started","app":"demo","resource":"web","instance":"web-1"}"#;
        assert_eq!(
            format_entry(entry),
            "2026-01-01T00:00:00.000001Z web-1 started"
        );
        let entry = br#"{"timestamp":"t","message":"m","app":"demo","resource":null}"#;
        assert_eq!(format_entry(entry), "t demo m", "a null is not a name");
        let entry = br#"{"timestamp":"t","message":"m","infra":"proxy"}"#;
        assert_eq!(format_entry(entry), "t proxy m");
    }

    // spec: CLI#log-display
    #[test]
    fn a_message_with_line_breaks_stays_on_one_line() {
        let entry = br#"{"timestamp":"t","message":"one\ntwo\r\n\u001b[31mthree","app":"demo"}"#;
        let line = format_entry(entry);
        assert!(!line.contains(['\n', '\r', '\u{1b}']), "{line:?}");
        assert_eq!(line, "t demo one\\ntwo\\r\\n\\u{1b}[31mthree");
    }

    // spec: CLI#log-display
    #[test]
    fn a_breadcrumb_is_marked_and_a_script_position_trails_it() {
        let entry = br#"{"timestamp":"t","message":"rt.start(web)","app":"demo","rt_call":"start","script_pos":"app.seed.rhai:12:3"}"#;
        assert_eq!(
            format_entry(entry),
            "t demo > rt.start(web)  (app.seed.rhai:12:3)"
        );
    }

    // spec: CLI#log-display
    #[test]
    fn a_line_that_is_not_a_json_object_is_printed_as_received() {
        for line in ["plain text", "42", r#""a string""#, "[1,2]"] {
            assert_eq!(format_entry(line.as_bytes()), line);
        }
    }
}
