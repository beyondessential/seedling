//! Per-key retry pacing: capped exponential back-off with no terminal state.
//!
//! Every retry site needs two things — a back-off, so failure does not mean
//! hammering, and a recovery path, so failure does not mean death. The audited
//! sites each had at most one. Image pulls retried immediately on every 5 s
//! tick and then set an `exhausted` flag nothing could ever clear, because
//! entries were only removed on a success that could no longer happen. TLS
//! issuance for Tailscale-discovered hostnames dispatched before the decision
//! that holds the debounce, so it opened and finalised a failed attempt row
//! every tick — and the 1000-row attempt cap then evicted *other* hostnames'
//! `last_attempt`, dissolving their debounce too.
//!
//! The semantics here are the ones `scheduler::should_back_off` already
//! implements and tests against the persisted operations log
//! (`r[history.operations.rate-limiting]`): capped exponential from a base
//! delay, with a gap longer than the cap resetting the count entirely. This is
//! the same shape for callers whose retry cadence is the reconciler tick
//! rather than a row in that log, so the two do not drift apart.
//!
//! **There is deliberately no exhausted state.** Past a threshold the caller
//! files an operator-visible fault and keeps attempting at the cap interval.
//! A permanent give-up is only legitimate behind an expiry or an explicit
//! operator action with a reset path — the TLS retry-block plus
//! `store::set_force_retry` pair is the model.

use std::{
    collections::HashMap,
    hash::Hash,
    time::{Duration, Instant},
};

/// Whether an operation may be attempted now, given how it has been failing.
#[derive(Debug, Clone)]
pub struct RetryGate {
    base: Duration,
    cap: Duration,
    failures: u32,
    last_failure: Option<Instant>,
}

impl RetryGate {
    pub fn new(base: Duration, cap: Duration) -> Self {
        Self {
            base,
            cap,
            failures: 0,
            last_failure: None,
        }
    }

    /// True when there is no failure history, when the back-off delay for the
    /// current failure count has elapsed, or when the gap since the last
    /// failure exceeds the cap — the staleness reset, which is what stops a
    /// long-idle key from inheriting an ancient streak.
    pub fn should_attempt(&self, now: Instant) -> bool {
        let Some(last) = self.last_failure else {
            return true;
        };
        let elapsed = now.saturating_duration_since(last);
        if elapsed >= self.cap {
            return true;
        }
        elapsed >= self.delay()
    }

    /// The current back-off delay: `base * 2^(failures - 1)`, capped.
    /// Saturating, so a very long streak cannot overflow into a short delay.
    pub fn delay(&self) -> Duration {
        if self.failures == 0 {
            return Duration::ZERO;
        }
        self.base
            .saturating_mul(2u32.saturating_pow(self.failures.saturating_sub(1)))
            .min(self.cap)
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }

    pub fn record_failure(&mut self, now: Instant) {
        self.failures = self.failures.saturating_add(1);
        self.last_failure = Some(now);
    }

    /// Full reset. Also the operator force-retry path.
    pub fn record_success(&mut self) {
        self.failures = 0;
        self.last_failure = None;
    }
}

/// A [`RetryGate`] per key, created on first use.
#[derive(Debug)]
pub struct RetryGates<K> {
    base: Duration,
    cap: Duration,
    gates: HashMap<K, RetryGate>,
}

impl<K: Eq + Hash> RetryGates<K> {
    pub fn new(base: Duration, cap: Duration) -> Self {
        Self {
            base,
            cap,
            gates: HashMap::new(),
        }
    }

    pub fn should_attempt(&self, key: &K, now: Instant) -> bool {
        self.gates
            .get(key)
            .is_none_or(|gate| gate.should_attempt(now))
    }

    pub fn failures(&self, key: &K) -> u32 {
        self.gates.get(key).map_or(0, RetryGate::failures)
    }

    /// Record a failure and return the resulting consecutive-failure count.
    pub fn record_failure(&mut self, key: K, now: Instant) -> u32 {
        let base = self.base;
        let cap = self.cap;
        let gate = self
            .gates
            .entry(key)
            .or_insert_with(|| RetryGate::new(base, cap));
        gate.record_failure(now);
        gate.failures()
    }

    /// Forget this key's history entirely.
    pub fn record_success(&mut self, key: &K) {
        self.gates.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: Duration = Duration::from_secs(5);
    const CAP: Duration = Duration::from_secs(300);

    fn gate() -> RetryGate {
        RetryGate::new(BASE, CAP)
    }

    // r[verify actuate.image.retry]
    #[test]
    fn delay_doubles_per_failure_and_caps() {
        let now = Instant::now();
        let mut gate = gate();
        assert!(gate.should_attempt(now), "no history, no wait");

        gate.record_failure(now);
        assert_eq!(gate.delay(), Duration::from_secs(5));
        gate.record_failure(now);
        assert_eq!(gate.delay(), Duration::from_secs(10));
        gate.record_failure(now);
        assert_eq!(gate.delay(), Duration::from_secs(20));

        for _ in 0..10 {
            gate.record_failure(now);
        }
        assert_eq!(gate.delay(), CAP, "delay is capped");
    }

    // r[verify actuate.image.retry]
    // The failure count is a u32 and a stuck key can accumulate for a long
    // time; 2^n must not wrap into a short delay.
    #[test]
    fn a_huge_failure_streak_stays_capped() {
        let now = Instant::now();
        let mut gate = gate();
        for _ in 0..99 {
            gate.record_failure(now);
        }
        assert_eq!(gate.delay(), CAP);
    }

    // r[verify actuate.image.retry]
    #[test]
    fn attempts_are_withheld_until_the_delay_elapses() {
        let now = Instant::now();
        let mut gate = gate();
        gate.record_failure(now);
        gate.record_failure(now);
        // Two failures → a 10 s delay.
        assert!(!gate.should_attempt(now));
        assert!(!gate.should_attempt(now + Duration::from_secs(9)));
        assert!(gate.should_attempt(now + Duration::from_secs(11)));
    }

    // r[verify actuate.image.retry]
    // Past the cap the streak resets, so a key that failed long ago and has
    // been quiet since does not inherit its old back-off.
    #[test]
    fn a_gap_beyond_the_cap_resets() {
        let now = Instant::now();
        let mut gate = gate();
        for _ in 0..20 {
            gate.record_failure(now);
        }
        assert!(!gate.should_attempt(now));
        assert!(gate.should_attempt(now + CAP + Duration::from_secs(1)));
    }

    // r[verify actuate.image.retry]
    // The point of the type: there is no state from which a key can never be
    // attempted again.
    #[test]
    fn no_failure_count_makes_a_key_permanently_ineligible() {
        let now = Instant::now();
        let mut gate = gate();
        for _ in 0..1000 {
            gate.record_failure(now);
        }
        assert!(
            gate.should_attempt(now + CAP),
            "however long it has been failing, waiting the cap must be enough"
        );
    }

    // r[verify actuate.image.retry]
    #[test]
    fn success_resets_fully() {
        let now = Instant::now();
        let mut gate = gate();
        for _ in 0..5 {
            gate.record_failure(now);
        }
        gate.record_success();
        assert_eq!(gate.failures(), 0);
        assert!(gate.should_attempt(now));
    }

    // r[verify actuate.image.retry]
    #[test]
    fn keys_are_paced_independently() {
        let now = Instant::now();
        let mut gates: RetryGates<&str> = RetryGates::new(BASE, CAP);
        gates.record_failure("broken", now);
        gates.record_failure("broken", now);

        assert!(!gates.should_attempt(&"broken", now));
        assert!(
            gates.should_attempt(&"healthy", now),
            "one key's failures must not pace another's"
        );
    }
}
