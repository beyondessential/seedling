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
                tokio::spawn(async move {
                    let result = driver.container.pull_image(&image_owned).await;
                    let mut map = pulling_map.lock();
                    if let Err(e) = result {
                        if let Some(state) = map.get_mut(&image_owned) {
                            state.in_flight = false;
                            let failures = {
                                state.gate.record_failure(Instant::now());
                                state.gate.failures()
                            };
                            // Past the threshold the failure is worth an
                            // operator's attention, but attempts continue at
                            // the cap interval: the fault is the escalation,
                            // not a terminal state. `image_pull_failed` is
                            // spec'd to clear on a subsequent success, which
                            // is only possible because we keep trying.
                            if failures >= PULL_FAULT_AFTER_ATTEMPTS {
                                tracing::error!(
                                    image = %image_owned,
                                    failures,
                                    delay = ?state.gate.delay(),
                                    error = %e,
                                    "image pull still failing; continuing to retry at the \
                                     back-off cap"
                                );
                            } else {
                                tracing::warn!(
                                    image = %image_owned,
                                    failures,
                                    delay = ?state.gate.delay(),
                                    "background image pull failed: {e}"
                                );
                            }
                        }
                    } else {
                        map.remove(&image_owned);
                    }
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
