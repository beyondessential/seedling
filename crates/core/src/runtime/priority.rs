use std::fmt;
use std::str::FromStr;

use rusqlite::params;
use seedling_protocol::names::AppName;

use crate::defs::enums::Priority;
use crate::runtime::db::Db;

/// The operator-set priority of an app: its standing relative to other apps on
/// a shared host when resources are contended. Distinct from a Deployment's own
/// [`Priority`](crate::defs::enums::Priority): app priority is an operational
/// decision, not a claim the definition makes for itself.
///
/// Ordered `High > Normal > Low`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AppPriority {
    High,
    #[default]
    Normal,
    Low,
}

impl AppPriority {
    /// Rank on the total order, higher meaning a stronger claim.
    pub fn rank(self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Normal => 1,
            Self::High => 2,
        }
    }

    /// Lower-case wire form used in `/apps/*` responses, event payloads, and
    /// the durable store.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Normal => "normal",
            Self::Low => "low",
        }
    }
}

impl fmt::Display for AppPriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Rejected when a caller supplies a level that is not one of the three the
/// system accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidAppPriority(pub String);

impl fmt::Display for InvalidAppPriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid app priority '{}': must be one of high, normal, low",
            self.0
        )
    }
}

impl std::error::Error for InvalidAppPriority {}

impl FromStr for AppPriority {
    type Err = InvalidAppPriority;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "high" => Ok(Self::High),
            "normal" => Ok(Self::Normal),
            "low" => Ok(Self::Low),
            other => Err(InvalidAppPriority(other.to_owned())),
        }
    }
}

// r[impl priority.app]
/// Load the stored app priority for an app. Returns `None` when no decision has
/// been stored, which means the app is at its default (`normal`).
pub fn load_app_priority(db: &Db, app: &AppName) -> rusqlite::Result<Option<AppPriority>> {
    let mut stmt = db
        .conn
        .prepare("SELECT priority FROM app_priorities WHERE app = ?1")?;
    let mut rows = stmt.query(params![app])?;
    match rows.next()? {
        Some(row) => {
            let stored: String = row.get(0)?;
            // A value written by an older or divergent build that no longer
            // parses is treated as the default rather than surfaced as a
            // definite level the operator never chose.
            Ok(Some(stored.parse().unwrap_or_default()))
        }
        None => Ok(None),
    }
}

// r[impl priority.settings]
/// Store the app priority for an app.
pub fn save_app_priority(db: &Db, app: &AppName, priority: AppPriority) -> rusqlite::Result<()> {
    let now = jiff::Timestamp::now().to_string();
    db.conn.execute(
        // r[impl history.persist.partial-update]
        "INSERT INTO app_priorities (app, priority, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(app) DO UPDATE SET
             priority = excluded.priority,
             updated_at = excluded.updated_at",
        params![app, priority.as_str(), now],
    )?;
    Ok(())
}

// i[impl app.priority.reset-on-uninstall]
/// Discard the stored app priority for an app (on uninstall or deregister), so
/// a later reinstall starts again at `normal`.
pub fn delete_app_priority_for_app(db: &Db, app: &AppName) -> rusqlite::Result<()> {
    db.conn
        .execute("DELETE FROM app_priorities WHERE app = ?1", params![app])?;
    Ok(())
}

// r[impl priority.app]
/// The effective app priority: the stored decision, or `normal` when none is
/// stored.
pub fn effective_app_priority(db: &Db, app: &AppName) -> rusqlite::Result<AppPriority> {
    Ok(load_app_priority(db, app)?.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Realising the ordering
// ---------------------------------------------------------------------------

/// Root slice holding every workload the runtime supervises.
pub const ROOT_SLICE: &str = "seedling.slice";

/// Slice the runtime's own infrastructure (proxy, resolver) sits under.
// r[impl priority.groups-owned]
pub const INFRA_SLICE: &str = "seedling-infra.slice";

/// The slice-name component reserved for [`INFRA_SLICE`]. An app whose name
/// realises this component would collide with it.
pub const INFRA_COMPONENT: &str = "infra";

/// CPU and I/O weight carried by [`INFRA_SLICE`]. Above every app weight: the
/// components that route traffic must keep running to reach whatever survives.
pub const INFRA_WEIGHT: u64 = 500;

/// Kill preference carried by each infrastructure unit. Below every value
/// [`oom_score_adjust`] can produce, so infra is the last thing shed.
// r[impl priority.kill-order]
pub const INFRA_OOM_SCORE_ADJUST: i32 = -900;

/// The app-name component of a slice name.
///
/// systemd reads `-` in a unit name as the slice hierarchy separator, so an app
/// called `a-b` would realise `seedling-a-b.slice`, which nests inside the slice
/// of an app called `a` and would take its weights. `_` cannot occur in an
/// [`AppName`], so mapping `-` onto it keeps distinct apps distinct while
/// leaving every app slice a direct child of the root.
pub fn slice_component(app: &AppName) -> String {
    app.as_str().replace('-', "_")
}

// r[impl priority.actuation]
/// The per-app slice, carrying the weight derived from the app priority.
pub fn app_slice(app: &AppName) -> String {
    format!("seedling-{}.slice", slice_component(app))
}

// r[impl priority.actuation]
/// The per-tier slice a Deployment's units join, nested under [`app_slice`] and
/// carrying the weight derived from the Deployment priority.
pub fn tier_slice(app: &AppName, deployment: Priority) -> String {
    format!(
        "seedling-{}-{}.slice",
        slice_component(app),
        deployment.as_str()
    )
}

// r[impl priority.scheduling]
/// Share of contended CPU and I/O an app claims relative to other apps.
pub fn app_weight(priority: AppPriority) -> u64 {
    match priority {
        AppPriority::High => 400,
        AppPriority::Normal => 100,
        AppPriority::Low => 25,
    }
}

// r[impl priority.scheduling]
/// Share of its app's contended capacity a Deployment claims relative to the
/// app's other Deployments.
pub fn deployment_weight(priority: Priority) -> u64 {
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

// r[impl priority.kill-order]
/// The kill preference for a workload of `deployment` priority in an app of
/// `app` priority. Higher means the kernel prefers it as a victim.
///
/// App-major by construction: the bands the app term produces are `[-700,-300]`
/// for `high`, `[-200,200]` for `normal` and `[300,700]` for `low`, and they do
/// not overlap. Every workload of a higher-priority app therefore outranks every
/// workload of a lower-priority one whatever either Deployment declares, so a
/// `Critical` Deployment in a `low` app is shed before a `Normal` Deployment in
/// a `high` app.
pub fn oom_score_adjust(app: AppPriority, deployment: Priority) -> i32 {
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
    (app_base + tier_offset).clamp(-1000, 1000)
}

/// Where one workload stands against every other on the host: its app's
/// priority and its own declared level. Every workload that is not a Deployment
/// stands at [`Priority::Normal`].
// r[impl priority.kill-order]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkloadStanding {
    pub app: AppPriority,
    pub deployment: Priority,
}

impl WorkloadStanding {
    pub fn new(app: AppPriority, deployment: Priority) -> Self {
        Self { app, deployment }
    }

    /// The slice this workload's unit joins.
    pub fn slice(&self, app: &AppName) -> String {
        tier_slice(app, self.deployment)
    }

    /// The kill preference this workload's unit carries.
    pub fn oom_score_adjust(&self) -> i32 {
        oom_score_adjust(self.app, self.deployment)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> AppName {
        AppName::new("myapp").unwrap()
    }

    // r[verify priority.app]
    #[test]
    fn load_returns_none_before_save() {
        let db = Db::open_in_memory().unwrap();
        assert!(load_app_priority(&db, &app()).unwrap().is_none());
    }

    // r[verify priority.app]
    #[test]
    fn effective_is_normal_without_a_decision() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(
            effective_app_priority(&db, &app()).unwrap(),
            AppPriority::Normal
        );
    }

    // r[verify priority.settings]
    #[test]
    fn save_then_load_round_trips() {
        let db = Db::open_in_memory().unwrap();
        save_app_priority(&db, &app(), AppPriority::High).unwrap();
        assert_eq!(
            load_app_priority(&db, &app()).unwrap(),
            Some(AppPriority::High)
        );
    }

    // r[verify priority.settings]
    #[test]
    fn save_overwrites_previous_decision() {
        let db = Db::open_in_memory().unwrap();
        save_app_priority(&db, &app(), AppPriority::High).unwrap();
        save_app_priority(&db, &app(), AppPriority::Low).unwrap();
        assert_eq!(
            load_app_priority(&db, &app()).unwrap(),
            Some(AppPriority::Low)
        );
    }

    // i[verify app.priority.reset-on-uninstall]
    #[test]
    fn delete_removes_the_decision() {
        let db = Db::open_in_memory().unwrap();
        save_app_priority(&db, &app(), AppPriority::High).unwrap();
        delete_app_priority_for_app(&db, &app()).unwrap();
        assert!(load_app_priority(&db, &app()).unwrap().is_none());
    }

    const ALL_APP: [AppPriority; 3] = [AppPriority::High, AppPriority::Normal, AppPriority::Low];
    const ALL_DEP: [Priority; 4] = [
        Priority::Critical,
        Priority::Elevated,
        Priority::Normal,
        Priority::Low,
    ];

    // r[verify priority.kill-order]
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

    // r[verify priority.kill-order]
    #[test]
    fn critical_in_a_low_app_is_shed_before_normal_in_a_high_app() {
        let critical_low = oom_score_adjust(AppPriority::Low, Priority::Critical);
        let normal_high = oom_score_adjust(AppPriority::High, Priority::Normal);
        assert!(critical_low > normal_high);
    }

    // r[verify priority.kill-order]
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

    // r[verify priority.kill-order]
    #[test]
    fn infra_outranks_every_app_workload() {
        for app in ALL_APP {
            for dep in ALL_DEP {
                assert!(INFRA_OOM_SCORE_ADJUST < oom_score_adjust(app, dep));
            }
        }
    }

    // r[verify priority.kill-order]
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

    // r[verify priority.scheduling]
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

    // r[verify priority.groups-owned]
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

    // r[verify priority.actuation]
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

    // r[verify priority.groups-owned]
    #[test]
    fn an_app_cannot_be_named_into_the_infra_slice_component() {
        // The guard for this lives in `reserved`; here we pin the collision it
        // exists to prevent, so the two cannot drift apart.
        let infra = AppName::new(INFRA_COMPONENT).unwrap();
        assert_eq!(app_slice(&infra), INFRA_SLICE);
    }

    #[test]
    fn parse_rejects_unknown_level() {
        assert!("urgent".parse::<AppPriority>().is_err());
        assert_eq!("high".parse::<AppPriority>().unwrap(), AppPriority::High);
    }
}
