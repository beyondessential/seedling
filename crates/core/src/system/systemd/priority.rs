//! How a standing is realised under systemd: the slices workloads are grouped
//! into, the share of contended capacity each slice claims, and how strongly
//! the kernel should prefer a workload as an out-of-memory victim.

use std::collections::BTreeMap;

use seedling_protocol::names::AppName;

use crate::defs::enums::Priority;
use crate::runtime::priority::AppPriority;
use crate::system::priority::{AppStandings, UnitStanding};

/// Slice the runtime's own infrastructure (proxy, resolver) sits under.
// spec: LINUX#resource-control-groups
pub const INFRA_SLICE: &str = "seedling-infra.slice";

/// CPU and I/O weight carried by [`INFRA_SLICE`]. Above every app weight: the
/// components that route traffic must keep running to reach whatever survives.
// spec: LINUX#resource-control-groups
pub const INFRA_WEIGHT: u64 = 500;

/// Kill preference carried by each infrastructure unit. Below every value
/// [`oom_score_adjust`] can produce, so infra is the last thing shed.
// spec: LINUX#kill-order
pub const INFRA_OOM_SCORE_ADJUST: i32 = -900;

/// A resource-control slice the runtime owns, with the weights it should carry.
// spec: LINUX#resource-control-groups
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceSpec {
    /// Full unit name, e.g. `seedling-myapp-critical.slice`.
    pub name: String,
    pub description: String,
    /// Share of contended capacity this slice claims, relative to its
    /// siblings. Fans out to both `CPUWeight=` and `IOWeight=`: a workload's
    /// standing is one thing, and nothing in the mapping distinguishes its
    /// claim on CPU from its claim on I/O.
    pub weight: u64,
}

/// The app-name component of a slice name.
///
/// systemd reads `-` in a unit name as the slice hierarchy separator, so an app
/// called `a-b` would realise `seedling-a-b.slice`, which nests inside the slice
/// of an app called `a` and would take its weights. `_` cannot occur in an
/// [`AppName`], so mapping `-` onto it keeps distinct apps distinct while
/// leaving every app slice a direct child of the root.
fn slice_component(app: &AppName) -> String {
    app.as_str().replace('-', "_")
}

// spec: LINUX#resource-control-groups
/// The per-app slice, carrying the weight derived from the app priority.
fn app_slice(app: &AppName) -> String {
    format!("seedling-{}.slice", slice_component(app))
}

// spec: LINUX#resource-control-groups
/// The per-tier slice a Deployment's units join, nested under [`app_slice`] and
/// carrying the weight derived from the Deployment priority.
fn tier_slice(app: &AppName, deployment: Priority) -> String {
    format!(
        "seedling-{}-{}.slice",
        slice_component(app),
        deployment.as_str()
    )
}

// spec: LINUX#resource-control-groups
/// Share of contended CPU and I/O an app claims relative to other apps.
fn app_weight(priority: AppPriority) -> u64 {
    match priority {
        AppPriority::High => 400,
        AppPriority::Normal => 100,
        AppPriority::Low => 25,
    }
}

// spec: LINUX#resource-control-groups
/// Share of its app's contended capacity a Deployment claims relative to the
/// app's other Deployments.
fn deployment_weight(priority: Priority) -> u64 {
    match priority {
        Priority::Critical => 400,
        Priority::Elevated => 200,
        Priority::Normal => 100,
        Priority::Low => 25,
    }
}

/// How far apart the app levels sit on the kill-preference scale. Wider than
/// the span any set of Deployment offsets covers, which is what makes the
/// ordering app-major: no Deployment level can lift a workload out of its app's
/// band.
const APP_OOM_STEP: i32 = 500;

/// How far a Deployment level moves a workload within its app's band.
const TIER_OOM_STEP: i32 = 100;

// spec: LINUX#kill-order
/// The kill preference for a workload of `deployment` priority in an app of
/// `app` priority. Higher means the kernel prefers it as a victim.
///
/// App-major by construction: the bands the app term produces are `[-700,-300]`
/// for `high`, `[-200,200]` for `normal` and `[300,700]` for `low`, and they do
/// not overlap. Every workload of a higher-priority app therefore outranks every
/// workload of a lower-priority one whatever either Deployment declares, so a
/// `Critical` Deployment in a `low` app is shed before a `Normal` Deployment in
/// a `high` app.
fn oom_score_adjust(app: AppPriority, deployment: Priority) -> i32 {
    let app_base = match app {
        AppPriority::High => -APP_OOM_STEP,
        AppPriority::Normal => 0,
        AppPriority::Low => APP_OOM_STEP,
    };
    let tier_offset = match deployment {
        Priority::Critical => -2 * TIER_OOM_STEP,
        Priority::Elevated => -TIER_OOM_STEP,
        Priority::Normal => 0,
        Priority::Low => 2 * TIER_OOM_STEP,
    };
    // Bounded by construction: the widest sum these two steps can produce is
    // ±700, well inside the kernel's ±1000. Left unclamped deliberately — a
    // clamp here would silently saturate a mis-set constant into a valid-looking
    // value, where `every_kill_preference_is_within_the_kernel_range` fails
    // loudly instead.
    app_base + tier_offset
}

/// The slice a unit of this standing joins. A workload joins the tier slice,
/// which carries the Deployment weight and nests under the app slice, which
/// carries the app weight.
// spec: LINUX#resource-control-groups
pub fn unit_slice(standing: &UnitStanding) -> String {
    match standing {
        UnitStanding::Infrastructure => INFRA_SLICE.to_owned(),
        UnitStanding::Workload { app, standing } => tier_slice(app, standing.deployment),
    }
}

/// The kill preference a unit of this standing carries.
// spec: LINUX#kill-order
pub fn unit_oom_score_adjust(standing: &UnitStanding) -> i32 {
    match standing {
        UnitStanding::Infrastructure => INFRA_OOM_SCORE_ADJUST,
        UnitStanding::Workload { standing, .. } => {
            oom_score_adjust(standing.app, standing.deployment)
        }
    }
}

/// The slices `apps` need, with the weights each should carry, and the
/// infrastructure slice.
// spec: LINUX#resource-control-groups
pub fn desired_slices(apps: &[AppStandings]) -> BTreeMap<String, SliceSpec> {
    let mut desired: BTreeMap<String, SliceSpec> = BTreeMap::new();

    for app in apps {
        let app_slice = app_slice(&app.app);
        desired.insert(
            app_slice.clone(),
            SliceSpec {
                name: app_slice,
                description: format!("seedling app {}", app.app),
                weight: app_weight(app.priority),
            },
        );
        for &tier in &app.levels {
            let name = tier_slice(&app.app, tier);
            desired.insert(
                name.clone(),
                SliceSpec {
                    name,
                    description: format!("seedling app {} at {}", app.app, tier.as_str()),
                    weight: deployment_weight(tier),
                },
            );
        }
    }

    // Inserted last so it always wins the key: an app named for the
    // infrastructure component realises the same slice name, and an app that
    // predates the reservation on that name would otherwise reweight the group
    // holding the proxy and the resolver down to its own standing.
    desired.insert(
        INFRA_SLICE.to_owned(),
        SliceSpec {
            name: INFRA_SLICE.to_owned(),
            description: "seedling infrastructure".to_owned(),
            weight: INFRA_WEIGHT,
        },
    );

    desired
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const ALL_APP: [AppPriority; 3] = [AppPriority::High, AppPriority::Normal, AppPriority::Low];
    const ALL_DEP: [Priority; 4] = [
        Priority::Critical,
        Priority::Elevated,
        Priority::Normal,
        Priority::Low,
    ];

    fn standings(app: &str, priority: AppPriority, levels: &[Priority]) -> AppStandings {
        AppStandings {
            app: AppName::new(app).unwrap(),
            priority,
            levels: levels.iter().copied().collect::<BTreeSet<_>>(),
        }
    }

    // spec: LINUX#kill-order
    // The property the whole ordering rests on: the app comparison decides
    // first. Every workload of a higher-priority app must be shed after every
    // workload of a lower-priority one, whatever either Deployment declares.
    #[test]
    fn kill_order_is_app_major() {
        for (higher, lower) in [
            (AppPriority::High, AppPriority::Normal),
            (AppPriority::Normal, AppPriority::Low),
            (AppPriority::High, AppPriority::Low),
        ] {
            let worst_in_higher = ALL_DEP
                .iter()
                .map(|&d| oom_score_adjust(higher, d))
                .max()
                .unwrap();
            let best_in_lower = ALL_DEP
                .iter()
                .map(|&d| oom_score_adjust(lower, d))
                .min()
                .unwrap();
            assert!(
                worst_in_higher < best_in_lower,
                "{higher} app's least-protected workload ({worst_in_higher}) must still \
                 outrank {lower} app's most-protected ({best_in_lower})"
            );
        }
    }

    // spec: LINUX#kill-order
    #[test]
    fn critical_in_a_low_app_is_shed_before_normal_in_a_high_app() {
        let critical_low = oom_score_adjust(AppPriority::Low, Priority::Critical);
        let normal_high = oom_score_adjust(AppPriority::High, Priority::Normal);
        assert!(critical_low > normal_high);
    }

    // spec: LINUX#kill-order
    // Within one app the declared level orders the workloads, lower level shed
    // first.
    #[test]
    fn within_an_app_the_deployment_level_orders_workloads() {
        for app in ALL_APP {
            assert!(
                oom_score_adjust(app, Priority::Critical)
                    < oom_score_adjust(app, Priority::Elevated)
            );
            assert!(
                oom_score_adjust(app, Priority::Elevated) < oom_score_adjust(app, Priority::Normal)
            );
            assert!(oom_score_adjust(app, Priority::Normal) < oom_score_adjust(app, Priority::Low));
        }
    }

    // spec: LINUX#kill-order
    #[test]
    fn infra_outranks_every_app_workload() {
        for app in ALL_APP {
            for dep in ALL_DEP {
                assert!(INFRA_OOM_SCORE_ADJUST < oom_score_adjust(app, dep));
            }
        }
    }

    // spec: LINUX#kill-order
    #[test]
    fn every_kill_preference_is_within_the_kernel_range() {
        for app in ALL_APP {
            for dep in ALL_DEP {
                let v = oom_score_adjust(app, dep);
                assert!((-1000..=1000).contains(&v), "{v} out of range");
            }
        }
        assert!((-1000..=1000).contains(&INFRA_OOM_SCORE_ADJUST));
    }

    // spec: LINUX#resource-control-groups
    #[test]
    fn weights_increase_with_priority_and_stay_in_the_supervisor_range() {
        assert!(app_weight(AppPriority::High) > app_weight(AppPriority::Normal));
        assert!(app_weight(AppPriority::Normal) > app_weight(AppPriority::Low));
        assert!(deployment_weight(Priority::Critical) > deployment_weight(Priority::Elevated));
        assert!(deployment_weight(Priority::Elevated) > deployment_weight(Priority::Normal));
        assert!(deployment_weight(Priority::Normal) > deployment_weight(Priority::Low));
        for w in ALL_APP.iter().map(|&p| app_weight(p)) {
            assert!((1..=10_000).contains(&w));
        }
        for w in ALL_DEP.iter().map(|&p| deployment_weight(p)) {
            assert!((1..=10_000).contains(&w));
        }
        assert!((1..=10_000).contains(&INFRA_WEIGHT));
        assert!(INFRA_WEIGHT > app_weight(AppPriority::High));
    }

    // spec: LINUX#resource-control-groups
    // A hyphen in an app name is the slice hierarchy separator, so it must not
    // survive into the component: `a-b` would otherwise nest inside `a`'s slice.
    #[test]
    fn slice_names_keep_distinct_apps_in_distinct_trees() {
        // Unescaped, `app-two` would realise `seedling-app-two.slice`, which
        // systemd nests inside `app`'s own slice and subjects to its weights.
        let hyphened = AppName::new("app-two").unwrap();
        let plain = AppName::new("app").unwrap();
        assert_eq!(app_slice(&hyphened), "seedling-app_two.slice");
        assert_eq!(app_slice(&plain), "seedling-app.slice");
        assert!(!slice_component(&hyphened).contains('-'));
        assert_ne!(app_slice(&hyphened), app_slice(&plain));
    }

    // spec: LINUX#resource-control-groups
    #[test]
    fn tier_slice_nests_under_its_app_slice() {
        let app = AppName::new("app-two").unwrap();
        let tier = tier_slice(&app, Priority::Critical);
        assert_eq!(tier, "seedling-app_two-critical.slice");
        // systemd derives the parent by trimming the last `-` component.
        let parent = format!(
            "{}.slice",
            tier.trim_end_matches(".slice").rsplit_once('-').unwrap().0
        );
        assert_eq!(parent, app_slice(&app));
    }

    // spec: LINUX#resource-control-groups
    #[test]
    fn an_app_cannot_be_named_into_the_infra_slice_component() {
        // The guard for this is the reserved app names; here we pin the
        // collision it exists to prevent, so the two cannot drift apart.
        let infra = AppName::new(crate::reserved::INFRA_APP_NAME).unwrap();
        assert_eq!(app_slice(&infra), INFRA_SLICE);
    }

    // spec: LINUX#resource-control-groups
    #[test]
    fn each_level_gets_a_tier_slice_under_its_app() {
        let slices = desired_slices(&[standings(
            "demo",
            AppPriority::Normal,
            &[Priority::Critical, Priority::Low, Priority::Normal],
        )]);

        assert!(slices.contains_key("seedling-demo.slice"));
        assert!(slices.contains_key("seedling-demo-critical.slice"));
        assert!(slices.contains_key("seedling-demo-low.slice"));
        assert!(slices.contains_key("seedling-demo-normal.slice"));
        assert!(!slices.contains_key("seedling-demo-elevated.slice"));
    }

    // spec: LINUX#resource-control-groups
    #[test]
    fn the_app_slice_carries_the_app_weight_and_the_tier_slice_the_tier_weight() {
        let slices = desired_slices(&[standings(
            "demo",
            AppPriority::High,
            &[Priority::Critical, Priority::Normal],
        )]);

        assert_eq!(
            slices["seedling-demo.slice"].weight,
            app_weight(AppPriority::High)
        );
        assert_eq!(
            slices["seedling-demo-critical.slice"].weight,
            deployment_weight(Priority::Critical)
        );
    }

    // spec: LINUX#resource-control-groups
    // The same levels at a different app priority must produce a different
    // app-slice weight: that difference is what a reweight applies.
    #[test]
    fn changing_the_app_priority_changes_the_app_slice_weight() {
        let high = desired_slices(&[standings("demo", AppPriority::High, &[Priority::Normal])]);
        let low = desired_slices(&[standings("demo", AppPriority::Low, &[Priority::Normal])]);
        assert_ne!(
            high["seedling-demo.slice"].weight,
            low["seedling-demo.slice"].weight
        );
        // The tier slices are unchanged: the app term lives on the app slice.
        assert_eq!(
            high["seedling-demo-normal.slice"].weight,
            low["seedling-demo-normal.slice"].weight
        );
    }

    // spec: LINUX#resource-control-groups
    #[test]
    fn the_infra_slice_is_always_present_and_outweighs_every_app() {
        let slices = desired_slices(&[]);
        assert_eq!(slices[INFRA_SLICE].weight, INFRA_WEIGHT);

        let with_app = desired_slices(&[standings("demo", AppPriority::High, &[Priority::Normal])]);
        assert!(with_app[INFRA_SLICE].weight > with_app["seedling-demo.slice"].weight);
    }

    // spec: LINUX#resource-control-groups
    // Registering an app under the infrastructure name is refused, but an app
    // that predates that reservation still reaches this derivation, and its
    // slice name is byte-identical to the infrastructure slice. The
    // infrastructure entry has to win, or the group holding the proxy and the
    // resolver takes the app's standing.
    #[test]
    fn an_app_named_for_the_infra_slice_cannot_displace_it() {
        let slices = desired_slices(&[standings(
            crate::reserved::INFRA_APP_NAME,
            AppPriority::Low,
            &[Priority::Normal],
        )]);
        assert_eq!(
            slices[INFRA_SLICE].weight, INFRA_WEIGHT,
            "infrastructure keeps its own weight, not the app's"
        );
    }

    // spec: LINUX#resource-control-groups
    // Two apps whose names differ only by a hyphen must not share a slice, nor
    // nest one inside the other.
    #[test]
    fn hyphenated_app_names_get_their_own_slice_trees() {
        let slices = desired_slices(&[
            standings("app", AppPriority::Normal, &[Priority::Normal]),
            standings("app-two", AppPriority::High, &[Priority::Normal]),
        ]);
        assert!(slices.contains_key("seedling-app.slice"));
        assert!(slices.contains_key("seedling-app_two.slice"));
        assert_ne!(
            slices["seedling-app.slice"].weight,
            slices["seedling-app_two.slice"].weight
        );
    }

    // spec: LINUX#kill-order
    #[test]
    fn a_workload_unit_takes_its_tier_slice_and_its_kill_preference() {
        let standing = UnitStanding::Workload {
            app: AppName::new("app-two").unwrap(),
            standing: crate::system::priority::WorkloadStanding::new(
                AppPriority::Low,
                Priority::Critical,
            ),
        };
        assert_eq!(unit_slice(&standing), "seedling-app_two-critical.slice");
        assert_eq!(
            unit_oom_score_adjust(&standing),
            oom_score_adjust(AppPriority::Low, Priority::Critical)
        );
        assert_eq!(unit_slice(&UnitStanding::Infrastructure), INFRA_SLICE);
        assert_eq!(
            unit_oom_score_adjust(&UnitStanding::Infrastructure),
            INFRA_OOM_SCORE_ADJUST
        );
    }
}
