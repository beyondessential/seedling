use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

/// Cooperative cancellation signal for a single in-flight operation.
///
/// Cancellation is observed by the action runtime at barrier entry points
/// (`check_barrier`, `do_stop`) and by the lifecycle loop while sleeping
/// between suspension cycles. Once requested, the signal is terminal — there
/// is no un-request.
///
/// Uses a `parking_lot::Condvar` rather than `tokio::sync::Notify` so that
/// the blocking lifecycle loop (which runs on a `spawn_blocking` thread) can
/// wait on it without bouncing through an async runtime.
// r[impl operation.cancel]
#[derive(Debug)]
pub struct CancelToken {
    requested: AtomicBool,
    mu: Mutex<()>,
    cv: Condvar,
}

impl Default for CancelToken {
    fn default() -> Self {
        Self {
            requested: AtomicBool::new(false),
            mu: Mutex::new(()),
            cv: Condvar::new(),
        }
    }
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a token that is already in the cancelled state. Used when the
    /// cancel flag was persisted across a daemon restart.
    pub fn pre_cancelled() -> Self {
        let t = Self::default();
        t.requested.store(true, Ordering::SeqCst);
        t
    }

    /// Flip the token to the cancelled state and wake every waiter.
    pub fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
        let _g = self.mu.lock();
        self.cv.notify_all();
    }

    pub fn is_cancelled(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// Block for up to `timeout`, returning early as soon as cancellation is
    /// requested.
    pub fn wait_for(&self, timeout: Duration) {
        if self.is_cancelled() {
            return;
        }
        let mut guard = self.mu.lock();
        if self.is_cancelled() {
            return;
        }
        self.cv.wait_for(&mut guard, timeout);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn new_token_not_cancelled() {
        let t = CancelToken::new();
        assert!(!t.is_cancelled());
    }

    #[test]
    fn request_flips_state() {
        let t = CancelToken::new();
        t.request();
        assert!(t.is_cancelled());
    }

    #[test]
    fn pre_cancelled_starts_cancelled() {
        let t = CancelToken::pre_cancelled();
        assert!(t.is_cancelled());
    }

    #[test]
    fn wait_for_returns_immediately_if_cancelled() {
        let t = CancelToken::pre_cancelled();
        let start = std::time::Instant::now();
        t.wait_for(Duration::from_secs(5));
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn wait_for_wakes_on_cancel() {
        let t = Arc::new(CancelToken::new());
        let t2 = Arc::clone(&t);
        let h = std::thread::spawn(move || {
            t2.wait_for(Duration::from_secs(5));
        });
        std::thread::sleep(Duration::from_millis(20));
        t.request();
        let start = std::time::Instant::now();
        h.join().unwrap();
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn wait_for_times_out_without_cancel() {
        let t = CancelToken::new();
        let start = std::time::Instant::now();
        t.wait_for(Duration::from_millis(30));
        assert!(start.elapsed() >= Duration::from_millis(25));
        assert!(!t.is_cancelled());
    }
}
