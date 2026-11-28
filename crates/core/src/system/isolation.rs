//! Whether the isolation rules are installed, and when to try installing them
//! again while they are not.
//!
//! Until they are installed, no workload container and no shell session may
//! start: a container on a pod network without them could reach every other
//! pod and the host. Installing them also removes the mount translations held
//! for shell sessions of an earlier run, which could otherwise match a prefix
//! given to a new pod and grant it that session's mounts.

use std::time::{Duration, Instant};

use crate::system::DataPlane;

/// The delay after the first failed install.
const FIRST_RETRY_DELAY: Duration = Duration::from_secs(5);

/// The longest delay between installs. Installing goes on at this spacing for
/// as long as it keeps failing.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(300);

/// Why the isolation rules are not installed: an install failed, or none has
/// been tried yet.
#[derive(Debug, Clone)]
pub struct IsolationMissing(String);

impl std::fmt::Display for IsolationMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the isolation rules are not installed: {}", self.0)
    }
}

impl std::error::Error for IsolationMissing {}

enum Standing {
    Installed,
    Missing {
        reason: String,
        /// The delay after the next failed install.
        delay: Duration,
        /// The earliest the next install may run.
        next_attempt: Instant,
    },
}

/// Whether the isolation rules are installed.
pub struct Isolation {
    // Held across an install, so concurrent callers wait for its outcome
    // rather than starting another.
    standing: tokio::sync::Mutex<Standing>,
}

impl Default for Isolation {
    fn default() -> Self {
        Self {
            standing: tokio::sync::Mutex::new(Standing::Missing {
                reason: "no install has been tried yet".to_owned(),
                delay: FIRST_RETRY_DELAY,
                next_attempt: Instant::now(),
            }),
        }
    }
}

impl Isolation {
    /// Install the rules through `data_plane`, and remove every shell
    /// session's mount translations, unless that is done or the delay since
    /// the last failed install has yet to pass. No session can register
    /// translations before this succeeds, so none of those removed is live.
    ///
    /// Each failed install doubles the delay up to a fixed maximum, at which
    /// it stays, so installing goes on for as long as it fails.
    pub(crate) async fn ensure(&self, data_plane: &dyn DataPlane) -> Result<(), IsolationMissing> {
        let mut standing = self.standing.lock().await;
        let delay = match &*standing {
            Standing::Installed => return Ok(()),
            Standing::Missing {
                reason,
                delay,
                next_attempt,
            } => {
                if Instant::now() < *next_attempt {
                    return Err(IsolationMissing(reason.clone()));
                }
                *delay
            }
        };
        let installed = match data_plane.ensure_isolation().await {
            Ok(()) => data_plane.reset_sessions().await.map_err(|e| {
                format!("removing the mount translations of earlier shell sessions failed: {e}")
                    .into()
            }),
            Err(e) => Err(e),
        };
        match installed {
            Ok(()) => {
                *standing = Standing::Installed;
                tracing::info!("the isolation rules are installed");
                Ok(())
            }
            Err(e) => {
                let reason = e.to_string();
                tracing::error!(
                    error = %reason,
                    retry_in = ?delay,
                    "installing the isolation rules failed; no workload or shell session \
                     starts until they are installed"
                );
                *standing = Standing::Missing {
                    reason: reason.clone(),
                    delay: (delay * 2).min(MAX_RETRY_DELAY),
                    next_attempt: Instant::now() + delay,
                };
                Err(IsolationMissing(reason))
            }
        }
    }

    /// Let the next install run at once, whatever the delay.
    #[cfg(test)]
    pub(crate) async fn retry_now(&self) {
        if let Standing::Missing { next_attempt, .. } = &mut *self.standing.lock().await {
            *next_attempt = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::stub::StubDataPlane;

    // spec: INFRA#pod-network-sources
    #[tokio::test]
    async fn a_failed_install_is_retried_after_a_growing_delay_and_stops_once_installed() {
        let plane = StubDataPlane::default();
        let isolation = Isolation::default();

        plane.refuse_isolation(true);
        assert!(isolation.ensure(&plane).await.is_err());
        assert!(
            isolation.ensure(&plane).await.is_err(),
            "still missing while the delay runs"
        );
        assert_eq!(plane.isolation_installs(), 1, "no install inside the delay");

        let delay = |isolation: &Isolation| match &*isolation.standing.try_lock().unwrap() {
            Standing::Missing { delay, .. } => *delay,
            Standing::Installed => panic!("not installed"),
        };
        assert_eq!(delay(&isolation), FIRST_RETRY_DELAY * 2);
        for _ in 0..10 {
            isolation.retry_now().await;
            assert!(isolation.ensure(&plane).await.is_err());
        }
        assert_eq!(
            delay(&isolation),
            MAX_RETRY_DELAY,
            "the delay stays at the cap"
        );

        plane.refuse_isolation(false);
        isolation.retry_now().await;
        assert!(isolation.ensure(&plane).await.is_ok());
        let installs = plane.isolation_installs();
        assert!(isolation.ensure(&plane).await.is_ok());
        assert_eq!(plane.isolation_installs(), installs, "installed once");
    }
}
