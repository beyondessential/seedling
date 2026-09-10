//! Shutdown-signal handling shared by the long-running commands.

/// Resolve when the process is asked to stop.
///
/// `i[ctl.graceful-shutdown]` and `i[ctl.logs.follow-interrupt]` require a
/// clean exit on SIGINT *or* SIGTERM. Every long-running command selected on
/// `ctrl_c()` alone, which is SIGINT only, so SIGTERM took its default
/// disposition and killed the process outright — no forward teardown, no
/// counter summary, no exit-code contract. `systemctl stop`, a container stop
/// and a supervisor shutdown all send SIGTERM, so the documented behaviour
/// was unreachable by the means most likely to be used.
///
/// Four commands needed this, so it lives here rather than being spelt out
/// four times with four chances to diverge.
// i[impl ctl.graceful-shutdown]
// i[impl ctl.logs.follow-interrupt]
pub(crate) async fn shutdown_requested() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        // A failure to install the SIGTERM handler must not lose SIGINT, so
        // fall back to waiting on SIGINT alone rather than returning.
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = sigterm.recv() => {}
                }
            }
            Err(e) => {
                tracing::warn!("cannot listen for SIGTERM: {e}; SIGINT only");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::shutdown_requested;

    // i[verify ctl.graceful-shutdown]
    // Raising a real SIGTERM here would risk killing the test binary if the
    // handler were not yet installed, so this pins the part that can regress
    // silently: that the wait actually waits. A helper that returned
    // immediately would tear every long-running command down at once.
    #[tokio::test]
    async fn the_wait_does_not_resolve_without_a_signal() {
        assert!(
            tokio::time::timeout(Duration::from_millis(150), shutdown_requested())
                .await
                .is_err(),
            "must keep waiting until a signal arrives"
        );
    }
}
