use std::{sync::Arc, time::Instant};

use snafu::ResultExt;

use super::{ActuateError, Actuator, ContainerSnafu, ImageUnavailableSnafu};

const MAX_PULL_ATTEMPTS: u32 = 5;

/// If a pull task has not completed after this duration, assume it is stuck
/// (hung podman, panicked task) and allow a fresh attempt.
const PULL_STALE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

pub(super) struct PullState {
    pub(super) started_at: Instant,
    pub(super) attempts: u32,
    /// `true` while the background tokio task is still running.
    /// Set to `false` on completion (success or failure) so the next tick
    /// can decide to retry without waiting for the stale timeout.
    pub(super) in_flight: bool,
    /// Set once `attempts` exceeds `MAX_PULL_ATTEMPTS`. Prevents the
    /// "exceeded max attempts" error from being logged every tick.
    pub(super) exhausted: bool,
}

impl Actuator {
    // r[impl reconciliation.liveness]
    /// Check image availability; spawn background pull if missing.
    /// Returns `Ok(())` if the image is locally present, or
    /// `Err(ActuateError::ImageUnavailable)` if a pull is in progress / exhausted.
    pub(crate) async fn ensure_image_available(&self, image: &str) -> Result<(), ActuateError> {
        if !self
            .driver
            .container
            .image_exists(image)
            .await
            .context(ContainerSnafu)?
        {
            let mut pulling = self.pulling.lock();
            let should_spawn = match pulling.get(image) {
                None => true,
                Some(state) if state.exhausted => false,
                Some(state) if !state.in_flight => {
                    // Previous attempt finished (failed); retry immediately.
                    true
                }
                Some(state) if state.started_at.elapsed() >= PULL_STALE_TIMEOUT => {
                    tracing::warn!(
                        image = %image,
                        elapsed = ?state.started_at.elapsed(),
                        attempts = state.attempts,
                        "in-flight image pull appears stale, resubmitting"
                    );
                    true
                }
                Some(_) => false,
            };
            if should_spawn {
                let attempts = pulling.get(image).map(|s| s.attempts + 1).unwrap_or(1);
                if attempts > MAX_PULL_ATTEMPTS {
                    tracing::error!(
                        image = %image,
                        attempts = attempts - 1,
                        "image pull exceeded max attempts, giving up"
                    );
                    pulling.insert(
                        image.to_owned(),
                        PullState {
                            started_at: Instant::now(),
                            attempts,
                            in_flight: false,
                            exhausted: true,
                        },
                    );
                } else {
                    pulling.insert(
                        image.to_owned(),
                        PullState {
                            started_at: Instant::now(),
                            attempts,
                            in_flight: true,
                            exhausted: false,
                        },
                    );
                    let driver = Arc::clone(&self.driver);
                    let image_owned = image.to_owned();
                    let pulling_map = Arc::clone(&self.pulling);
                    tokio::spawn(async move {
                        let result = driver.container.pull_image(&image_owned).await;
                        let mut map = pulling_map.lock();
                        if let Err(e) = result {
                            tracing::warn!(
                                image = %image_owned,
                                error = %e,
                                "background image pull failed"
                            );
                            if let Some(state) = map.get_mut(&image_owned) {
                                state.in_flight = false;
                            }
                        } else {
                            map.remove(&image_owned);
                        }
                    });
                }
            }
            return ImageUnavailableSnafu {
                reference: image.to_owned(),
            }
            .fail();
        }
        Ok(())
    }
}
