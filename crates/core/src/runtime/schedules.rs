use jiff::{SignedDuration, Timestamp};

use crate::defs::action::parse_cron_expr;
use crate::runtime::barrier::OperationId;
use crate::runtime::db::{self, Db};
use crate::runtime::scheduler::{ScheduleResult, Scheduler};

pub struct FiredSchedule {
    pub app: String,
    pub action: String,
    pub accepted: bool,
    pub operation_id: Option<OperationId>,
    pub generation: u64,
}

// r[impl schedule.tick]
// r[impl schedule.fire]
// r[impl schedule.catch-up]
pub fn check_due_schedules(
    db: &Db,
    scheduler: &mut Scheduler,
    now: Timestamp,
    app_generations: &dyn Fn(&str) -> Option<u64>,
) -> Vec<FiredSchedule> {
    let rows = match db::list_all_schedules(db) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("failed to list schedules: {e}");
            return Vec::new();
        }
    };

    let mut fired: Vec<FiredSchedule> = Vec::new();
    for row in rows {
        let crontab = match parse_cron_expr(&row.cronexpr, &row.app, &row.action) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    app = %row.app,
                    action = %row.action,
                    cronexpr = %row.cronexpr,
                    "invalid cron expression in schedule table: {e}"
                );
                continue;
            }
        };

        let base_time = match &row.last_fired_at {
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
                tracing::warn!(
                    app = %row.app,
                    action = %row.action,
                    "failed to compute next fire time: {e}"
                );
                continue;
            }
        };

        // Fire whenever the next scheduled boundary is at or before now. No
        // lower bound: if the daemon missed the boundary window, we still
        // fire once to catch up. Because last_fired_at is updated to `now`
        // on fire, find_next() on the next tick returns the next future
        // boundary, so we do not fire repeatedly for older missed windows.
        if next_fire <= now {
            let generation = app_generations(&row.app).unwrap_or(0);
            let result = scheduler.request(
                &row.app,
                &row.action,
                serde_json::Map::new(),
                generation,
                generation,
                "schedule",
            );

            let accepted = matches!(result, ScheduleResult::Accepted);
            match result {
                ScheduleResult::Accepted | ScheduleResult::Queued => {
                    let fired_at = now.to_string();
                    if let Err(e) = db::upsert_schedule_fired(
                        db,
                        &row.app,
                        &row.action,
                        &row.cronexpr,
                        &fired_at,
                    ) {
                        tracing::error!(
                            app = %row.app,
                            action = %row.action,
                            "failed to update last_fired_at: {e}"
                        );
                    }
                    let op_id = if accepted {
                        scheduler.active().map(|a| a.operation_id.clone())
                    } else {
                        scheduler
                            .queue_iter()
                            .find(|q| q.app == row.app)
                            .map(|q| q.operation_id.clone())
                    };
                    // r[impl schedule.audit]
                    tracing::info!(
                        app = %row.app,
                        action = %row.action,
                        trigger = "schedule",
                        "scheduled action fired"
                    );
                    fired.push(FiredSchedule {
                        app: row.app.clone(),
                        action: row.action.clone(),
                        accepted,
                        operation_id: op_id,
                        generation,
                    });
                }
                ScheduleResult::Rejected(_) => {
                    tracing::debug!(
                        app = %row.app,
                        action = %row.action,
                        "schedule fire rejected (operation in progress or queued)"
                    );
                }
            }
        }
    }

    fired
}

/// Wrapper for the main loop to call periodically. Tracks its own last-check
/// timestamp so it runs at most once per minute.
pub struct ScheduleTicker {
    last_check: Option<Timestamp>,
}

impl Default for ScheduleTicker {
    fn default() -> Self {
        Self::new()
    }
}

impl ScheduleTicker {
    pub fn new() -> Self {
        Self { last_check: None }
    }

    // r[impl schedule.tick]
    /// Returns `Some(now)` if it is time to run a schedule tick, updating
    /// internal state. The caller should then pass `now` to
    /// `check_due_schedules`. Returns `None` if the interval has not elapsed.
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
    use crate::runtime::db::Db;

    // r[verify schedule.state]
    #[test]
    fn schedule_table_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        let pairs = vec![
            ("backup".to_owned(), "0 2 * * *".to_owned()),
            ("cleanup".to_owned(), "*/15 * * * *".to_owned()),
        ];
        db::ensure_schedules(&db, "myapp", &pairs).unwrap();

        let rows = db::list_schedules(&db, "myapp").unwrap();
        assert_eq!(rows.len(), 2);

        db::upsert_schedule_fired(&db, "myapp", "backup", "0 2 * * *", "2026-01-01T02:00:00Z")
            .unwrap();

        let rows = db::list_schedules(&db, "myapp").unwrap();
        let backup_row = rows.iter().find(|r| r.action == "backup").unwrap();
        assert_eq!(
            backup_row.last_fired_at.as_deref(),
            Some("2026-01-01T02:00:00Z")
        );
    }

    // r[verify schedule.prune]
    #[test]
    fn prune_removes_stale_schedules() {
        let db = Db::open_in_memory().unwrap();
        let pairs = vec![
            ("backup".to_owned(), "0 2 * * *".to_owned()),
            ("cleanup".to_owned(), "*/15 * * * *".to_owned()),
        ];
        db::ensure_schedules(&db, "myapp", &pairs).unwrap();

        let valid = vec![("backup".to_owned(), "0 2 * * *".to_owned())];
        db::prune_schedules(&db, "myapp", &valid).unwrap();

        let rows = db::list_schedules(&db, "myapp").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, "backup");
    }

    // r[verify schedule.fire]
    #[test]
    fn check_due_fires_overdue_schedule() {
        let db = Db::open_in_memory().unwrap();

        let pairs = vec![("backup".to_owned(), "* * * * *".to_owned())];
        db::ensure_schedules(&db, "myapp", &pairs).unwrap();

        let now: Timestamp = "2026-04-18T12:01:00Z".parse().unwrap();
        db::upsert_schedule_fired(&db, "myapp", "backup", "* * * * *", "2026-04-18T12:00:00Z")
            .unwrap();

        let mut scheduler = Scheduler::new();
        let fired = check_due_schedules(&db, &mut scheduler, now, &|_| Some(1));
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].app, "myapp");
        assert_eq!(fired[0].action, "backup");
        assert!(fired[0].accepted);
    }

    // r[verify schedule.catch-up]
    // A schedule whose next fire time sits well in the past (because the
    // daemon was offline through the cron boundary) must fire once on the
    // next tick, rather than staying stuck forever because the old 59 s
    // fire-window excluded it.
    #[test]
    fn check_due_catches_up_long_missed_schedule() {
        let db = Db::open_in_memory().unwrap();

        let pairs = vec![("backup".to_owned(), "0 * * * *".to_owned())];
        db::ensure_schedules(&db, "myapp", &pairs).unwrap();

        // Last fired ~25 hours ago; next cron boundary from that point is
        // yesterday's 18:00, which is well outside any 59 s or 5-minute
        // window centred on `now`.
        db::upsert_schedule_fired(&db, "myapp", "backup", "0 * * * *", "2026-04-20T17:05:00Z")
            .unwrap();

        let now: Timestamp = "2026-04-21T18:37:22Z".parse().unwrap();
        let mut scheduler = Scheduler::new();
        let fired = check_due_schedules(&db, &mut scheduler, now, &|_| Some(1));
        assert_eq!(fired.len(), 1, "missed hourly schedule must catch up");

        // After firing we record last_fired_at = now, so the immediate next
        // check sees the next boundary still in the future.
        let fired_again = check_due_schedules(&db, &mut scheduler, now, &|_| Some(1));
        assert!(
            fired_again.is_empty(),
            "second check at same instant must not refire"
        );
    }

    // r[verify schedule.start-reject]
    #[test]
    fn on_schedule_rejects_start_action() {
        use crate::defs::action::Action;
        use rhai::{CustomType, Engine};

        let mut engine = Engine::new();
        engine.build_type::<Action>();

        let action = Action::new("start".to_owned(), "testapp".to_owned());
        let mut scope = rhai::Scope::new();
        scope.push("action", action);

        let result = engine
            .eval_with_scope::<rhai::Dynamic>(&mut scope, r#"action.on_schedule("* * * * *")"#);
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("start"),
            "error should mention start: {err_str}"
        );
    }
}
