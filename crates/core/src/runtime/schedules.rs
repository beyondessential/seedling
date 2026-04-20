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
// r[impl schedule.startup-grace]
pub fn check_due_schedules(
    db: &Db,
    scheduler: &mut Scheduler,
    now: Timestamp,
    is_startup: bool,
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

        // r[impl schedule.fire]
        let window_secs = if is_startup && is_infrequent_schedule(&crontab, now) {
            // r[impl schedule.startup-grace]
            300
        } else {
            59
        };

        let deadline = now;
        let window_start = deadline
            .checked_sub(SignedDuration::from_secs(window_secs))
            .unwrap_or(deadline);

        if next_fire > window_start && next_fire <= deadline {
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

/// Heuristic: a schedule is "infrequent" if the gap between the two next fire
/// times from `now` is >= 10 minutes. This avoids computing the full period
/// from the cron expression.
fn is_infrequent_schedule(crontab: &cronexpr::Crontab, now: Timestamp) -> bool {
    let first = match crontab.find_next(now) {
        Ok(z) => z,
        Err(_) => return false,
    };
    let first_ts = Timestamp::from(first);
    let second = match crontab.find_next(first_ts) {
        Ok(z) => z,
        Err(_) => return false,
    };
    let second_ts = Timestamp::from(second);
    let ten_minutes_later = first_ts
        .checked_add(SignedDuration::from_mins(10))
        .unwrap_or(first_ts);
    second_ts >= ten_minutes_later
}

/// Wrapper for the main loop to call periodically. Tracks its own last-check
/// timestamp so it runs at most once per minute.
pub struct ScheduleTicker {
    last_check: Option<Timestamp>,
    is_startup: bool,
}

impl Default for ScheduleTicker {
    fn default() -> Self {
        Self::new()
    }
}

impl ScheduleTicker {
    pub fn new() -> Self {
        Self {
            last_check: None,
            is_startup: true,
        }
    }

    // r[impl schedule.tick]
    /// Returns `Some((now, is_startup))` if it is time to run a schedule tick,
    /// updating internal state. The caller should then pass those parameters to
    /// `check_due_schedules`. Returns `None` if the interval has not elapsed.
    pub fn maybe_tick(&mut self) -> Option<(Timestamp, bool)> {
        let now = Timestamp::now();
        if let Some(last) = self.last_check {
            let threshold = last
                .checked_add(SignedDuration::from_secs(60))
                .unwrap_or(last);
            if now < threshold {
                return None;
            }
        }
        let is_startup = self.is_startup;
        self.last_check = Some(now);
        self.is_startup = false;
        Some((now, is_startup))
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
        let fired = check_due_schedules(&db, &mut scheduler, now, false, &|_| Some(1));
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].app, "myapp");
        assert_eq!(fired[0].action, "backup");
        assert!(fired[0].accepted);
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
