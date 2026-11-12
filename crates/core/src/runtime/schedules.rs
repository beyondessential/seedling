use std::collections::BTreeMap;
use std::sync::Arc;

use jiff::{SignedDuration, Timestamp};
use parking_lot::Mutex;
use seedling_protocol::names::{ActionName, AppName, ParamName};

use crate::defs::action::parse_cron_expr;
use crate::defs::install::ParamDef;
use crate::runtime::apps::AppPhase;
use crate::runtime::barrier::OperationId;
use crate::runtime::db::{self, Db};
use crate::runtime::faults::{self, FaultKey, FaultMeta};
use crate::runtime::scheduler::{ScheduleResult, Scheduler};

/// What firing a schedule needs to know about its app: the generation to fire
/// against, the phase the scheduler decides admission on, and the param schema
/// of each of its actions, for validating a run.
///
/// The phase travels as a handle rather than a value because the scheduler
/// reads it under its own lock, after the caller looked the app up.
pub type ScheduledApp = (
    u64,
    Arc<Mutex<AppPhase>>,
    Arc<BTreeMap<ActionName, BTreeMap<ParamName, ParamDef>>>,
);

pub struct FiredSchedule {
    pub app: AppName,
    pub action: ActionName,
    pub accepted: bool,
    pub operation_id: Option<OperationId>,
    pub generation: u64,
    /// The expression the schedule fired on, for stamping its fire time once
    /// a queued fire is persisted.
    pub cronexpr: String,
}

// spec: SCHED#firing
// spec: SCHED#fire-times
///
/// `app_generations` returns `Some(generation)` for apps in the `Installed`
/// phase and `None` for any other state (NotInstalled, Installing,
/// Uninstalling, or unknown app). This is the gate for
/// `SCHED#firing`: a schedule is only fired when the
/// callback returns a generation, so that stuck install attempts and
/// in-progress uninstalls don't keep monopolising the scheduler slot.
pub fn check_due_schedules(
    db: &Db,
    scheduler: &mut Scheduler,
    now: Timestamp,
    installed_app: &dyn Fn(&AppName) -> Option<ScheduledApp>,
) -> Vec<FiredSchedule> {
    // spec: SCHED#firing
    // With the records unreadable nothing is fired: an absent schedule and an
    // unread one must not look alike.
    let unreadable = FaultKey::new(
        &AppName::new_unchecked("_system"),
        "schedules_unreadable",
        "",
    );
    let rows = match db::list_all_schedules(db) {
        Ok(r) => {
            let _ = faults::clear_fault_by_key(db, &unreadable);
            r
        }
        Err(e) => {
            tracing::error!("failed to list schedules: {e}");
            if let Err(e) = faults::file_once(
                db,
                &unreadable,
                &FaultMeta::default(),
                &format!("the schedule records cannot be read, so no schedule is fired: {e}"),
            ) {
                tracing::warn!("failed to file schedules_unreadable: {e}");
            }
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

        // spec: SCHED#firing
        // A last fire time that cannot be parsed is not "never fired": the
        // schedule is not fired, and the action holds a fault until it can be.
        let unreadable = FaultKey::new(&row.app, "schedule_unreadable", row.action.as_str());
        let base_time = match &row.last_fired_at {
            Some(ts_str) => match ts_str.parse::<Timestamp>() {
                Ok(ts) => ts,
                Err(e) => {
                    tracing::warn!(
                        app = %row.app,
                        action = %row.action,
                        last_fired_at = %ts_str,
                        "schedule not fired: its last fire time cannot be parsed: {e}"
                    );
                    if let Err(e) = faults::file_once(
                        db,
                        &unreadable,
                        &FaultMeta::default(),
                        &format!(
                            "the last fire time {ts_str:?} of a schedule of {} cannot be parsed, \
                             so it is not fired",
                            row.action
                        ),
                    ) {
                        tracing::warn!(app = %row.app, action = %row.action, "failed to file schedule_unreadable: {e}");
                    }
                    continue;
                }
            },
            None => now
                .checked_sub(SignedDuration::from_secs(300))
                .unwrap_or(now),
        };
        let _ = faults::clear_fault_by_key(db, &unreadable);

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
            // spec: SCHED#firing
            let Some((generation, phase, schemas)) = installed_app(&row.app) else {
                tracing::debug!(
                    app = %row.app,
                    action = %row.action,
                    "schedule fire skipped (app is not in Installed phase)"
                );
                continue;
            };
            // spec: ACN#scheduled-actions
            // A scheduled run supplies no params, so each declared param takes
            // its default; a run that fails that is not made, and says so.
            let key = FaultKey::new(&row.app, "schedule_params_invalid", row.action.as_str());
            let validation = validate_empty_invocation(schemas.get(&row.action));
            let stamp_fire = |db: &Db| {
                if let Err(e) = db::upsert_schedule_fired(
                    db,
                    &row.app,
                    &row.action,
                    &row.cronexpr,
                    &now.to_string(),
                ) {
                    tracing::error!(
                        app = %row.app,
                        action = %row.action,
                        "failed to update last_fired_at: {e}"
                    );
                }
            };
            match validation {
                Err(why) => {
                    let meta = FaultMeta::default();
                    let description = format!(
                        "scheduled run of {} not made: it fails param validation with no params \
                         supplied: {why}",
                        row.action
                    );
                    if let Err(e) = faults::file_once(db, &key, &meta, &description) {
                        tracing::warn!(app = %row.app, action = %row.action, "failed to file schedule_params_invalid: {e}");
                    }
                    // The run was due and is accounted for, so the next one
                    // is the next boundary rather than another failure now.
                    stamp_fire(db);
                    continue;
                }
                Ok(()) => {
                    let _ = faults::clear_fault_by_key(db, &key);
                }
            }

            // spec: TRD#uninstall-admission
            // The phase behind `installed_app` was read before the scheduler
            // was locked, so an uninstall can have landed in between. The
            // scheduler reads it again under its own lock.
            let result = scheduler.admit(
                &row.app,
                &row.action,
                serde_json::Map::new(),
                generation,
                generation,
                "schedule",
                &phase,
            );

            let accepted = matches!(result, ScheduleResult::Accepted);
            match result {
                ScheduleResult::Accepted | ScheduleResult::Queued => {
                    // spec: SCHED#fire-times
                    // Stamp the fire when it happens, not when it is intended.
                    // A queued fire is stamped by the caller once it is
                    // persisted: until then a restart would lose the
                    // operation while the database said it had fired. While it
                    // stays queued the next check's re-fire is rejected as
                    // SameAppAlreadyQueued, so it cannot double-fire; once
                    // persisted and stamped, a restart restores it from the
                    // queue instead of firing again.
                    if accepted {
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
                    }
                    let op_id = if accepted {
                        scheduler.active().map(|a| a.operation_id.clone())
                    } else {
                        scheduler
                            .queue_iter()
                            .find(|q| q.app == row.app)
                            .map(|q| q.operation_id.clone())
                    };
                    // spec: SCHED#firing
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
                        cronexpr: row.cronexpr.clone(),
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

/// Check an action's param schema against an invocation that supplies no
/// params, as a scheduled run is.
// spec: ACN#scheduled-actions
fn validate_empty_invocation(schema: Option<&BTreeMap<ParamName, ParamDef>>) -> Result<(), String> {
    let Some(schema) = schema else {
        return Ok(());
    };
    let mut params = serde_json::Map::new();
    crate::runtime::action_params::apply_schema(schema, &mut params).map_err(|e| e.to_string())
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

    // spec: SCHED#firing
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
    /// An app the scheduler will admit, for a test driving the firing logic
    /// rather than the phase gate.
    fn installed() -> Arc<Mutex<AppPhase>> {
        Arc::new(Mutex::new(AppPhase::Installed))
    }

    fn no_schemas() -> Arc<BTreeMap<ActionName, BTreeMap<ParamName, ParamDef>>> {
        Arc::default()
    }

    fn required_text_param() -> BTreeMap<ParamName, ParamDef> {
        BTreeMap::from([(
            ParamName::new("target").unwrap(),
            ParamDef {
                kind: crate::defs::install::ParamKind::Text,
                required: true,
                default_value: None,
                description: None,
                secret: None,
            },
        )])
    }

    use super::*;
    use crate::runtime::db::Db;

    fn app_name(s: &str) -> AppName {
        AppName::new(s).unwrap()
    }

    fn action_name(s: &str) -> ActionName {
        ActionName::new(s).unwrap()
    }

    // spec: SCHED#schedule-records
    #[test]
    fn schedule_table_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        let pairs = vec![
            (action_name("backup"), "0 2 * * *".to_owned()),
            (action_name("cleanup"), "*/15 * * * *".to_owned()),
        ];
        db::ensure_schedules(&db, &app_name("myapp"), &pairs).unwrap();

        let rows = db::list_schedules(&db, &app_name("myapp")).unwrap();
        assert_eq!(rows.len(), 2);

        db::upsert_schedule_fired(
            &db,
            &app_name("myapp"),
            &action_name("backup"),
            "0 2 * * *",
            "2026-01-01T02:00:00Z",
        )
        .unwrap();

        let rows = db::list_schedules(&db, &app_name("myapp")).unwrap();
        let backup_row = rows.iter().find(|r| r.action == "backup").unwrap();
        assert_eq!(
            backup_row.last_fired_at.as_deref(),
            Some("2026-01-01T02:00:00Z")
        );
    }

    // spec: SCHED#schedule-records
    #[test]
    fn prune_removes_stale_schedules() {
        let db = Db::open_in_memory().unwrap();
        let pairs = vec![
            (action_name("backup"), "0 2 * * *".to_owned()),
            (action_name("cleanup"), "*/15 * * * *".to_owned()),
        ];
        db::ensure_schedules(&db, &app_name("myapp"), &pairs).unwrap();

        let valid = vec![(action_name("backup"), "0 2 * * *".to_owned())];
        db::prune_schedules(&db, &app_name("myapp"), &valid).unwrap();

        let rows = db::list_schedules(&db, &app_name("myapp")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, "backup");
    }

    // spec: SCHED#firing
    #[test]
    fn check_due_fires_overdue_schedule() {
        let db = Db::open_in_memory().unwrap();

        let pairs = vec![(action_name("backup"), "* * * * *".to_owned())];
        db::ensure_schedules(&db, &app_name("myapp"), &pairs).unwrap();

        let now: Timestamp = "2026-04-18T12:01:00Z".parse().unwrap();
        db::upsert_schedule_fired(
            &db,
            &app_name("myapp"),
            &action_name("backup"),
            "* * * * *",
            "2026-04-18T12:00:00Z",
        )
        .unwrap();

        let mut scheduler = Scheduler::new();
        let fired = check_due_schedules(&db, &mut scheduler, now, &|_| {
            Some((1, installed(), no_schemas()))
        });
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].app, "myapp");
        assert_eq!(fired[0].action, "backup");
        assert!(fired[0].accepted);
    }

    // spec: SCHED#firing
    #[test]
    fn a_last_fire_time_that_cannot_be_parsed_is_not_never_fired() {
        let db = Db::open_in_memory().unwrap();
        let pairs = vec![(action_name("backup"), "* * * * *".to_owned())];
        db::ensure_schedules(&db, &app_name("myapp"), &pairs).unwrap();
        db.conn
            .execute("UPDATE action_schedules SET last_fired_at = 'garbage'", [])
            .unwrap();

        let now: Timestamp = "2026-04-18T12:01:00Z".parse().unwrap();
        let mut scheduler = Scheduler::new();
        let fired = check_due_schedules(&db, &mut scheduler, now, &|_| {
            Some((1, installed(), no_schemas()))
        });
        assert!(fired.is_empty(), "an unreadable schedule must not fire");
        let held = faults::list_active_faults(&db, Some(&app_name("myapp"))).unwrap();
        assert!(
            held.iter()
                .any(|f| f.kind == "schedule_unreadable" && f.subject == "backup"),
            "{held:?}"
        );

        // Once it reads again, it fires and the fault clears.
        db.conn
            .execute(
                "UPDATE action_schedules SET last_fired_at = '2026-04-18T12:00:00Z'",
                [],
            )
            .unwrap();
        let fired = check_due_schedules(&db, &mut scheduler, now, &|_| {
            Some((1, installed(), no_schemas()))
        });
        assert_eq!(fired.len(), 1);
        assert!(
            faults::list_active_faults(&db, Some(&app_name("myapp")))
                .unwrap()
                .iter()
                .all(|f| f.kind != "schedule_unreadable")
        );
    }

    // spec: SCHED#firing
    #[test]
    fn unreadable_schedule_records_fire_nothing_and_hold_a_host_fault() {
        let db = Db::open_in_memory().unwrap();
        let pairs = vec![(action_name("backup"), "* * * * *".to_owned())];
        db::ensure_schedules(&db, &app_name("myapp"), &pairs).unwrap();
        db.conn
            .execute_batch("ALTER TABLE action_schedules RENAME TO gone;")
            .unwrap();

        let now: Timestamp = "2026-04-18T12:01:00Z".parse().unwrap();
        let mut scheduler = Scheduler::new();
        let fired = check_due_schedules(&db, &mut scheduler, now, &|_| {
            Some((1, installed(), no_schemas()))
        });
        assert!(fired.is_empty());
        let system = AppName::new_unchecked("_system");
        let held = faults::list_active_faults(&db, Some(&system)).unwrap();
        assert!(
            held.iter().any(|f| f.kind == "schedules_unreadable"),
            "{held:?}"
        );

        db.conn
            .execute_batch("ALTER TABLE gone RENAME TO action_schedules;")
            .unwrap();
        check_due_schedules(&db, &mut scheduler, now, &|_| {
            Some((1, installed(), no_schemas()))
        });
        assert!(
            faults::list_active_faults(&db, Some(&system))
                .unwrap()
                .iter()
                .all(|f| f.kind != "schedules_unreadable")
        );
    }

    // spec: SCHED#fire-times
    // A schedule whose next fire time sits well in the past (because the
    // daemon was offline through the cron boundary) must fire once on the
    // next tick, rather than staying stuck forever because the old 59 s
    // fire-window excluded it.
    #[test]
    fn check_due_catches_up_long_missed_schedule() {
        let db = Db::open_in_memory().unwrap();

        let pairs = vec![(action_name("backup"), "0 * * * *".to_owned())];
        db::ensure_schedules(&db, &app_name("myapp"), &pairs).unwrap();

        // Last fired ~25 hours ago; next cron boundary from that point is
        // yesterday's 18:00, which is well outside any 59 s or 5-minute
        // window centred on `now`.
        db::upsert_schedule_fired(
            &db,
            &app_name("myapp"),
            &action_name("backup"),
            "0 * * * *",
            "2026-04-20T17:05:00Z",
        )
        .unwrap();

        let now: Timestamp = "2026-04-21T18:37:22Z".parse().unwrap();
        let mut scheduler = Scheduler::new();
        let fired = check_due_schedules(&db, &mut scheduler, now, &|_| {
            Some((1, installed(), no_schemas()))
        });
        assert_eq!(fired.len(), 1, "missed hourly schedule must catch up");

        // After firing we record last_fired_at = now, so the immediate next
        // check sees the next boundary still in the future.
        let fired_again = check_due_schedules(&db, &mut scheduler, now, &|_| {
            Some((1, installed(), no_schemas()))
        });
        assert!(
            fired_again.is_empty(),
            "second check at same instant must not refire"
        );
    }

    // spec: ACN#scheduled-actions
    // spec: FLT#app-faults
    #[test]
    fn a_scheduled_run_that_fails_validation_with_no_params_is_not_made_and_says_so() {
        let db = Db::open_in_memory().unwrap();
        let pairs = vec![(action_name("backup"), "0 * * * *".to_owned())];
        db::ensure_schedules(&db, &app_name("myapp"), &pairs).unwrap();
        db::upsert_schedule_fired(
            &db,
            &app_name("myapp"),
            &action_name("backup"),
            "0 * * * *",
            "2026-04-21T17:05:00Z",
        )
        .unwrap();
        let now: Timestamp = "2026-04-21T18:37:22Z".parse().unwrap();
        let mut scheduler = Scheduler::new();

        // A required param with no default cannot be satisfied by no params.
        let schemas = Arc::new(BTreeMap::from([(
            action_name("backup"),
            required_text_param(),
        )]));
        let fired = check_due_schedules(&db, &mut scheduler, now, &|_| {
            Some((1, installed(), Arc::clone(&schemas)))
        });
        assert!(fired.is_empty(), "the closure is not run");
        assert!(scheduler.active().is_none());
        let filed = faults::list_active_faults(&db, Some(&app_name("myapp"))).unwrap();
        assert_eq!(filed.len(), 1);
        assert_eq!(filed[0].kind, "schedule_params_invalid");
        assert_eq!(filed[0].subject, "backup", "the action is the subject");

        // The fire is recorded, so the next check is not another failure.
        check_due_schedules(&db, &mut scheduler, now, &|_| {
            Some((1, installed(), Arc::clone(&schemas)))
        });
        assert_eq!(
            faults::list_active_faults(&db, Some(&app_name("myapp")))
                .unwrap()
                .len(),
            1
        );

        // A later run that passes clears it: here the param gains a default.
        let mut with_default = required_text_param();
        with_default
            .get_mut(&ParamName::new("target").unwrap())
            .unwrap()
            .default_value = Some("all".to_owned());
        let schemas = Arc::new(BTreeMap::from([(action_name("backup"), with_default)]));
        let later: Timestamp = "2026-04-21T19:37:22Z".parse().unwrap();
        let fired = check_due_schedules(&db, &mut scheduler, later, &|_| {
            Some((1, installed(), Arc::clone(&schemas)))
        });
        assert_eq!(fired.len(), 1, "the declared default satisfies the schema");
        assert!(
            faults::list_active_faults(&db, Some(&app_name("myapp")))
                .unwrap()
                .is_empty()
        );
    }

    // spec: ACN#start-action
    #[test]
    fn on_schedule_rejects_start_action() {
        use crate::defs::action::Action;
        use rhai::{CustomType, Engine};

        let mut engine = Engine::new();
        engine.build_type::<Action>();

        let action = Action::new(action_name("start"), app_name("testapp"));
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
