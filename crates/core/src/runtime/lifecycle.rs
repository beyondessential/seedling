use serde::{Deserialize, Serialize};

// l[impl rt.lifecyle]
// r[impl lifecycle.states]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum LifecycleState {
    Pending,
    Scheduled,
    Running,
    Ready,
    Terminating,
    Terminated,
    Unscheduled,
}

impl LifecycleState {
    // r[impl lifecycle.transitions]
    pub fn can_transition_to(self, next: LifecycleState) -> bool {
        use LifecycleState::*;
        matches!(
            (self, next),
            // Normal forward
            (Pending, Scheduled) | (Scheduled, Running) | (Running, Ready) |
            (Ready, Terminating) | (Terminating, Terminated) | (Terminated, Unscheduled) |
            // Skips — forward within the active portion
            (Pending, Running) | (Pending, Ready) | (Pending, Terminated) |
            (Scheduled, Ready) | (Scheduled, Terminated) |
            (Running, Terminating) | (Running, Terminated) | (Ready, Terminated) |
            // Skips — any active state directly to Unscheduled (force-remove)
            (Pending, Unscheduled) | (Scheduled, Unscheduled) |
            (Running, Unscheduled) | (Ready, Unscheduled) |
            (Terminating, Unscheduled)
        )
    }

    // r[impl lifecycle.derivation]
    /// Returns true if self has reached or passed `required` in the lifecycle order.
    pub fn has_reached(self, required: LifecycleState) -> bool {
        fn ordinal(s: LifecycleState) -> u8 {
            match s {
                LifecycleState::Pending => 0,
                LifecycleState::Scheduled => 1,
                LifecycleState::Running => 2,
                LifecycleState::Ready => 3,
                LifecycleState::Terminating => 4,
                LifecycleState::Terminated => 5,
                LifecycleState::Unscheduled => 6,
            }
        }
        ordinal(self) >= ordinal(required)
    }
}

#[cfg(test)]
mod tests {
    use super::LifecycleState::*;

    // r[verify lifecycle.transitions]
    #[test]
    fn valid_transitions() {
        assert!(Pending.can_transition_to(Scheduled));
        assert!(Scheduled.can_transition_to(Running));
        assert!(Running.can_transition_to(Ready));
        assert!(Ready.can_transition_to(Terminating));
        assert!(Terminating.can_transition_to(Terminated));
        assert!(Terminated.can_transition_to(Unscheduled));
    }

    // r[verify lifecycle.transitions]
    #[test]
    fn skip_transitions() {
        assert!(Running.can_transition_to(Terminated));
        assert!(Ready.can_transition_to(Terminated));
        assert!(Pending.can_transition_to(Terminated));
        assert!(Scheduled.can_transition_to(Terminated));
        assert!(Terminating.can_transition_to(Unscheduled));
    }

    // r[verify lifecycle.transitions]
    #[test]
    fn invalid_transitions() {
        assert!(!Unscheduled.can_transition_to(Running));
        assert!(!Terminated.can_transition_to(Running));
        assert!(!Terminated.can_transition_to(Pending));
        assert!(!Ready.can_transition_to(Pending));
        assert!(!Running.can_transition_to(Pending));
    }

    // r[verify lifecycle.derivation]
    // r[verify lifecycle.states]
    #[test]
    fn has_reached() {
        assert!(!Pending.has_reached(Running));
        assert!(!Scheduled.has_reached(Running));
        assert!(Running.has_reached(Running));
        assert!(Ready.has_reached(Running));
        assert!(Terminated.has_reached(Running));
        assert!(Terminated.has_reached(Terminated));
        assert!(!Pending.has_reached(Terminated));
        // Terminated has "passed through" Ready
        assert!(Terminated.has_reached(Ready));
    }
}
