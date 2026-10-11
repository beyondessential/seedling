//! Shared helpers for the Windows runtime spike harnesses.
//!
//! Each binary in this crate is a draft experiment for one of the spikes in
//! `docs/plans/windows-spike-*.md`. The harnesses are deliberately small and
//! print their observations to stdout so a spike run can be pasted back into
//! the corresponding plan file's "Exit criteria" section.

use std::error::Error;

/// Result alias used across the harnesses. Both `windows::core::Error` and
/// `std::io::Error` convert into this via `?`.
pub type Outcome = Result<(), Box<dyn Error>>;

/// Print a numbered experiment step heading (mirrors the plan files).
pub fn step(n: u32, title: &str) {
    println!("\n[{n}] {title}");
}

/// Print an observation under the current step.
pub fn observe(msg: impl AsRef<str>) {
    println!("    - {}", msg.as_ref());
}

/// Print a value the plan asks to be recorded (latency, digest, verdict).
pub fn record(key: &str, value: impl std::fmt::Display) {
    println!("    * {key}: {value}");
}

/// Stub for a sub-experiment not yet drafted, naming the Win32 surface it needs
/// so the gap is explicit rather than a silent no-op (see AGENTS.md).
#[macro_export]
macro_rules! not_yet {
    ($($arg:tt)*) => {
        todo!(concat!("spike step not yet drafted: ", $($arg)*))
    };
}
