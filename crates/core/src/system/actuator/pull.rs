use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use snafu::ResultExt;

use super::{ActuateError, Actuator, ContainerSnafu, ImageUnavailableSnafu};
use crate::runtime::retry::RetryGate;

/// Consecutive failures after which the pull becomes an operator-visible
/// fault. Attempts continue at the cap interval past this point.
pub(super) const PULL_FAULT_AFTER_ATTEMPTS: u32 = 5;

/// Retry pacing for image pulls. The reconciler ticks every five seconds, so
/// the base matches the tick and the cap bounds a broken registry to one
/// attempt every five minutes.
const PULL_BACKOFF_BASE: Duration = Duration::from_secs(5);
const PULL_BACKOFF_CAP: Duration = Duration::from_secs(300);

/// If a pull task has not completed after this duration, assume it is stuck
/// (hung podman, panicked task) and allow a fresh attempt.
const PULL_STALE_TIMEOUT: Duration = Duration::from_secs(300);

pub(super) struct PullState {
    pub(super) started_at: Instant,
    /// `true` while the background tokio task is still running.
    /// Set to `false` on completion (success or failure) so the next tick
    /// can decide to retry without waiting for the stale timeout.
    pub(super) in_flight: bool,
    /// Paces the retries and counts consecutive failures.
    ///
    /// This replaces an `attempts` counter and an `exhausted` flag. The flag
    /// was terminal and nothing could ever clear it: entries were removed
    /// from the map only on a successful pull, which could no longer be
    /// attempted once the flag was set. A registry that was briefly
    /// unreachable therefore disabled actuation of the workload permanently,
    /// until the daemon restarted.
    // r[impl actuate.image.retry]
    pub(super) gate: RetryGate,
}

impl PullState {
    fn new() -> Self {
        Self {
            started_at: Instant::now(),
            in_flight: false,
            gate: RetryGate::new(PULL_BACKOFF_BASE, PULL_BACKOFF_CAP),
        }
    }
}

/// Apply a finished pull to the shared state.
///
/// `attempt` is the `started_at` the caller was spawned with. A resubmitted
/// stale pull leaves the earlier task running, and that task finishing says
/// nothing about the attempt now in flight: crediting its failure would clear
/// `in_flight` for the live attempt and let the next tick spawn a third pull,
/// which is the hammering this back-off exists to stop.
///
/// A success is unconditional — the image is present however it got there.
// r[impl actuate.image.retry]
fn finish_pull(
    map: &mut std::collections::HashMap<String, PullState>,
    image: &str,
    attempt: Instant,
    error: Option<String>,
) {
    let Some(e) = error else {
        map.remove(image);
        return;
    };
    let Some(state) = map.get_mut(image) else {
        return;
    };
    if state.started_at != attempt {
        tracing::debug!(
            image = %image,
            "a superseded image pull failed; the live attempt is unaffected: {e}"
        );
        return;
    }
    state.in_flight = false;
    state.gate.record_failure(Instant::now());
    let failures = state.gate.failures();
    // Past the threshold the failure is worth an operator's attention, but
    // attempts continue at the cap interval: the fault is the escalation, not
    // a terminal state. `image_pull_failed` is spec'd to clear on a subsequent
    // success, which is only possible because we keep trying.
    if failures >= PULL_FAULT_AFTER_ATTEMPTS {
        tracing::error!(
            image = %image,
            failures,
            delay = ?state.gate.delay(),
            error = %e,
            "image pull still failing; continuing to retry at the back-off cap"
        );
    } else {
        tracing::warn!(
            image = %image,
            failures,
            delay = ?state.gate.delay(),
            "background image pull failed: {e}"
        );
    }
}

impl Actuator {
    // r[impl reconciliation.liveness]
    // r[impl actuate.image.retry]
    /// Check image availability; spawn background pull if missing.
    /// Returns `Ok(())` if the image is locally present, or
    /// `Err(ActuateError::ImageUnavailable)` if a pull is in progress or
    /// backing off.
    pub(crate) async fn ensure_image_available(&self, image: &str) -> Result<(), ActuateError> {
        if !self
            .driver
            .container
            .image_exists(image)
            .await
            .context(ContainerSnafu)?
        {
            let now = Instant::now();
            let mut pulling = self.pulling.lock();
            let should_spawn = match pulling.get(image) {
                None => true,
                // A previous attempt finished and failed. Retry when its
                // back-off has elapsed — not on the very next tick, which is
                // what turned an unreachable registry into a hammering loop.
                Some(state) if !state.in_flight => state.gate.should_attempt(now),
                Some(state) if state.started_at.elapsed() >= PULL_STALE_TIMEOUT => {
                    tracing::warn!(
                        image = %image,
                        elapsed = ?state.started_at.elapsed(),
                        failures = state.gate.failures(),
                        "in-flight image pull appears stale, resubmitting"
                    );
                    true
                }
                Some(_) => false,
            };
            if should_spawn {
                let state = pulling
                    .entry(image.to_owned())
                    .or_insert_with(PullState::new);
                state.started_at = now;
                state.in_flight = true;

                let driver = Arc::clone(&self.driver);
                let image_owned = image.to_owned();
                let pulling_map = Arc::clone(&self.pulling);
                // The stale path can leave two tasks running for one image.
                // The attempt this task owns is identified by the instant it
                // was spawned at, so its completion cannot be mistaken for the
                // other's.
                let attempt = now;
                tokio::spawn(async move {
                    let result = driver.container.pull_image(&image_owned).await;
                    let mut map = pulling_map.lock();
                    finish_pull(
                        &mut map,
                        &image_owned,
                        attempt,
                        result.err().map(|e| e.to_string()),
                    );
                });
            }
            return ImageUnavailableSnafu {
                reference: image.to_owned(),
            }
            .fail();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn state_in_flight(at: Instant) -> PullState {
        let mut s = PullState::new();
        s.started_at = at;
        s.in_flight = true;
        s
    }

    // r[verify actuate.image.retry]
    #[test]
    fn a_superseded_pull_failing_does_not_disturb_the_live_attempt() {
        let stale = Instant::now();
        let live = stale + Duration::from_secs(PULL_STALE_TIMEOUT.as_secs() + 1);
        let mut map = HashMap::new();
        map.insert("img".to_owned(), state_in_flight(live));

        finish_pull(&mut map, "img", stale, Some("registry unreachable".into()));

        let state = map.get("img").expect("the entry survives");
        assert!(
            state.in_flight,
            "a superseded attempt must not clear in_flight for the one still running, or the \
             next tick spawns a third pull alongside it"
        );
        assert_eq!(
            state.gate.failures(),
            0,
            "nor may it pace the live attempt's back-off"
        );
    }

    // r[verify actuate.image.retry]
    #[test]
    fn the_live_attempt_failing_clears_in_flight_and_paces_the_retry() {
        let live = Instant::now();
        let mut map = HashMap::new();
        map.insert("img".to_owned(), state_in_flight(live));

        finish_pull(&mut map, "img", live, Some("registry unreachable".into()));

        let state = map.get("img").expect("the entry survives a failure");
        assert!(!state.in_flight);
        assert_eq!(state.gate.failures(), 1);
    }

    // r[verify actuate.image.retry]
    #[test]
    fn a_success_drops_the_entry_whichever_attempt_won() {
        let live = Instant::now();
        let stale = live - Duration::from_secs(PULL_STALE_TIMEOUT.as_secs() + 1);
        let mut map = HashMap::new();
        map.insert("img".to_owned(), state_in_flight(live));

        finish_pull(&mut map, "img", stale, None);

        assert!(
            !map.contains_key("img"),
            "the image is present however it got there"
        );
    }
}
