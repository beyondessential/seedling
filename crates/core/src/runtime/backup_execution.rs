use jiff::{SignedDuration, Timestamp};
use seedling_protocol::names::{ActionName, AppName, BackupStrategyName};

use crate::defs::action::parse_cron_expr;
use crate::runtime::backup_strategies;
use crate::runtime::db::Db;

// r[impl backup.execution]
// r[impl backup.execution.startup-cleanup]
/// Site-volume name prefix used for the read-only snapshots created by
/// `backup.execution`. Shared between the runtime path that creates and
/// removes them and the daemon's startup orphan-cleanup so both agree on
/// what counts as one of these snapshots.
pub const SNAPSHOT_NAME_PREFIX: &str = "backup-snap-";

pub struct DueStrategy {
    pub name: BackupStrategyName,
    pub via: AppName,
    pub schedule: String,
    pub volumes: Vec<String>,
}

// r[impl backup.schedule]
pub fn schedule_to_cronexpr(schedule: &str) -> Option<&'static str> {
    match schedule {
        "every hour" => Some("0 * * * *"),
        "twice a day" => Some("0 0,12 * * *"),
        "every day" => Some("0 0 * * *"),
        _ => None,
    }
}

// r[impl backup.schedule.delay]
pub fn random_delay_secs(schedule: &str) -> u64 {
    use rand_core::{OsRng, RngCore};
    let interval_secs: u64 = match schedule {
        "every hour" => 3600,
        "twice a day" => 43200,
        "every day" => 86400,
        _ => 3600,
    };
    let max_delay = interval_secs / 10;
    if max_delay == 0 {
        return 0;
    }
    OsRng.next_u64() % (max_delay + 1)
}

// r[impl backup.execution]
// r[impl backup.schedule.catch-up]
pub fn check_due_strategies(db: &Db, now: Timestamp) -> Vec<DueStrategy> {
    let strategies = match backup_strategies::list_all(db) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to list backup strategies for tick: {e}");
            return Vec::new();
        }
    };

    let mut due = Vec::new();

    for strategy in strategies {
        let Some(cronexpr_str) = schedule_to_cronexpr(&strategy.schedule) else {
            continue;
        };

        let backup_app = AppName::new_unchecked("backup");
        let strategy_action = ActionName::new_unchecked(strategy.name.as_str());
        let crontab = match parse_cron_expr(cronexpr_str, &backup_app, &strategy_action) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(strategy = %strategy.name, "invalid cron for backup schedule: {e}");
                continue;
            }
        };

        let base_time = match &strategy.last_fired_at {
            Some(ts_str) => match ts_str.parse::<Timestamp>() {
                Ok(ts) => ts,
                Err(_) => now
                    .checked_sub(SignedDuration::from_secs(300))
                    .unwrap_or(now),
            },
            None => now
                .checked_sub(SignedDuration::from_secs(300))
                .unwrap_or(now),
        };

        let next_fire = match crontab.find_next(base_time) {
            Ok(zoned) => Timestamp::from(zoned),
            Err(e) => {
                tracing::warn!(strategy = %strategy.name, "failed to compute next backup fire time: {e}");
                continue;
            }
        };

        // Fire whenever the next scheduled boundary is at or before now. No
        // lower bound: if the daemon missed the boundary window, we still
        // fire once to catch up. Because last_fired_at is updated to `now`
        // on fire, the next find_next() call returns the next future
        // boundary, so we do not fire repeatedly for older missed windows.
        if next_fire <= now {
            let fired_at = now.to_string();
            if let Err(e) = backup_strategies::update_last_fired_at(db, &strategy.name, &fired_at) {
                tracing::error!(
                    strategy = %strategy.name,
                    "failed to update backup last_fired_at: {e}"
                );
                continue;
            }
            tracing::info!(strategy = %strategy.name, "backup strategy due, firing");
            due.push(DueStrategy {
                name: strategy.name,
                via: strategy.via,
                schedule: strategy.schedule,
                volumes: strategy.volumes,
            });
        }
    }

    due
}

pub struct BackupTicker {
    last_check: Option<Timestamp>,
}

impl Default for BackupTicker {
    fn default() -> Self {
        Self::new()
    }
}

impl BackupTicker {
    pub fn new() -> Self {
        Self { last_check: None }
    }

    // r[impl backup.execution]
    /// Returns `Some(now)` if it is time to run a backup tick, updating
    /// internal state. The caller should then call `check_due_strategies(db,
    /// now)` on the DB thread. Returns `None` if the interval has not elapsed.
    pub fn maybe_tick(&mut self) -> Option<Timestamp> {
        let now = Timestamp::now();
        if let Some(last) = self.last_check {
            let threshold = last
                .checked_add(SignedDuration::from_secs(60))
                .unwrap_or(last);
            if now < threshold {
                return None;
            }
        }
        self.last_check = Some(now);
        Some(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{backup_strategies, db::Db};

    fn create_strategy(db: &Db, schedule: &str) {
        backup_strategies::create(
            db,
            &backup_strategies::BackupStrategy {
                name: BackupStrategyName::new("nightly").unwrap(),
                via: AppName::new("backup-kopia-s3").unwrap(),
                schedule: schedule.to_owned(),
                volumes: vec!["myapp/data".to_owned()],
                last_fired_at: None,
            },
        )
        .expect("create strategy");
    }

    // r[impl backup.schedule.catch-up]
    // r[verify backup.schedule]
    // r[verify backup.schedule.catch-up]
    // If the daemon was down through a scheduled fire time, the strategy fires
    // once to catch up instead of staying stuck on a past boundary forever.
    #[test]
    fn missed_fire_catches_up() {
        let db = Db::open_in_memory().expect("open in-memory db");
        create_strategy(&db, "every hour");

        // Simulate a strategy that last fired 25 hours ago — the daemon has
        // been down through many hourly boundaries.
        let long_ago = "2026-04-20T12:34:56Z";
        backup_strategies::update_last_fired_at(
            &db,
            &BackupStrategyName::new("nightly").unwrap(),
            long_ago,
        )
        .unwrap();

        let now: Timestamp = "2026-04-21T13:47:22Z".parse().unwrap();
        let due = check_due_strategies(&db, now);
        assert_eq!(due.len(), 1, "missed hourly fire should catch up once");
        assert_eq!(due[0].name, "nightly");

        // The catch-up pass must update last_fired_at to now, so a second
        // tick a few seconds later does not re-fire.
        let still = check_due_strategies(&db, now);
        assert!(
            still.is_empty(),
            "second check at same instant must not refire"
        );
    }

    // r[verify backup.schedule]
    #[test]
    fn fires_at_scheduled_boundary() {
        let db = Db::open_in_memory().expect("open in-memory db");
        create_strategy(&db, "every hour");
        backup_strategies::update_last_fired_at(
            &db,
            &BackupStrategyName::new("nightly").unwrap(),
            "2026-04-21T12:00:12Z",
        )
        .unwrap();

        // Just before 13:00: nothing due.
        let before: Timestamp = "2026-04-21T12:59:45Z".parse().unwrap();
        assert!(check_due_strategies(&db, before).is_empty());

        // A moment after 13:00: fire.
        let after: Timestamp = "2026-04-21T13:00:30Z".parse().unwrap();
        assert_eq!(check_due_strategies(&db, after).len(), 1);
    }

    // r[verify backup.schedule]
    // r[verify backup.run.last-fired]
    #[test]
    fn fire_updates_last_fired_at_to_now() {
        let db = Db::open_in_memory().expect("open in-memory db");
        create_strategy(&db, "every hour");

        let now: Timestamp = "2026-04-21T13:00:30Z".parse().unwrap();
        let due = check_due_strategies(&db, now);
        assert_eq!(due.len(), 1);

        let rows = backup_strategies::list_all(&db).unwrap();
        let updated = rows.iter().find(|s| s.name == "nightly").unwrap();
        assert_eq!(
            updated.last_fired_at.as_deref(),
            Some(now.to_string().as_str())
        );
    }

    // r[verify backup.schedule.delay]
    #[test]
    fn random_delay_is_within_bounds_for_named_schedules() {
        for _ in 0..64 {
            let d = random_delay_secs("every hour");
            assert!(d <= 360, "hourly delay should be 0..=360, got {d}");
            let d = random_delay_secs("twice a day");
            assert!(d <= 4320, "twice-daily delay should be 0..=4320, got {d}");
            let d = random_delay_secs("every day");
            assert!(d <= 8640, "daily delay should be 0..=8640, got {d}");
        }
    }

    // r[verify backup.schedule]
    #[test]
    fn schedule_to_cronexpr_maps_known_buckets() {
        assert_eq!(schedule_to_cronexpr("every hour"), Some("0 * * * *"));
        assert_eq!(schedule_to_cronexpr("twice a day"), Some("0 0,12 * * *"));
        assert_eq!(schedule_to_cronexpr("every day"), Some("0 0 * * *"));
        assert_eq!(schedule_to_cronexpr("nonsense"), None);
    }

    // r[verify backup.schedule]
    #[test]
    fn no_refire_within_same_window() {
        let db = Db::open_in_memory().expect("open in-memory db");
        create_strategy(&db, "every hour");
        // Just after the top of the hour.
        let fire_time: Timestamp = "2026-04-21T13:00:12Z".parse().unwrap();
        assert_eq!(check_due_strategies(&db, fire_time).len(), 1);

        // Another tick 40s later: must not re-fire for the same 13:00 boundary.
        let later: Timestamp = "2026-04-21T13:00:52Z".parse().unwrap();
        assert!(check_due_strategies(&db, later).is_empty());
    }
}
