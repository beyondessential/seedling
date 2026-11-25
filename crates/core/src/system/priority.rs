//! Where a workload stands against every other on the host.
//!
//! The levels themselves are declared in BSL ([`Priority`]) and set by the
//! operator ([`AppPriority`]); a standing combines the two for one workload.
//! The process manager realises standings as resource-control groups and kill
//! preferences, as its platform settles them.
//!
//! Kept apart from the durable setting in [`crate::runtime::priority`] because
//! the two answer different questions and change for different reasons: that
//! module is about what the operator chose and how it is stored, this one is
//! about what the process manager is told.

use std::collections::BTreeSet;

use seedling_protocol::names::AppName;

use crate::defs::enums::Priority;
use crate::runtime::priority::AppPriority;

/// Where one workload stands against every other on the host: its app's
/// priority and its own declared level. Every workload that is not a Deployment
/// stands at [`Priority::Normal`].
// spec: PRIO#kill-order
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkloadStanding {
    pub app: AppPriority,
    pub deployment: Priority,
}

impl WorkloadStanding {
    pub fn new(app: AppPriority, deployment: Priority) -> Self {
        Self { app, deployment }
    }
}

/// Where a supervised unit stands: the runtime's own infrastructure, which
/// ranks above every app workload, or a workload of an app.
// spec: PRIO#kill-order
// spec: PRIO#resource-control-groups
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitStanding {
    Infrastructure,
    Workload {
        app: AppName,
        standing: WorkloadStanding,
    },
}

/// The standings one app's workloads take: the app's priority, and every
/// Deployment level its workloads stand at.
// spec: PRIO#resource-control-groups
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppStandings {
    pub app: AppName,
    pub priority: AppPriority,
    pub levels: BTreeSet<Priority>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: CONST#priority
    // The order is derived from the declaration order, so a reorder that looks
    // cosmetic would silently invert it. The spec states this order explicitly.
    #[test]
    fn the_levels_are_totally_ordered_strongest_first() {
        assert!(Priority::Critical > Priority::Elevated);
        assert!(Priority::Elevated > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
        assert_eq!(Priority::default(), Priority::Normal);
    }
}
