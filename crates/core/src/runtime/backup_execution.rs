use jiff::{SignedDuration, Timestamp};

use crate::defs::action::parse_cron_expr;
use crate::runtime::backup_strategies;
use crate::runtime::db::Db;

pub struct DueStrategy {
    pub name: String,
    pub via: String,
    pub schedule: String,
    pub volumes: Vec<String>,
}

// r[impl backup.schedule]
// r[impl backup.execution]
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
pub fn check_due_strategies(db: &Db, now: Timestamp) -> Vec<DueStrategy> {
    let strategies = match backup_strategies::list_all(db) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to list backup strategies for tick: {e}");
            return Vec::new();
        }
    };

    let window_start = now
        .checked_sub(SignedDuration::from_secs(59))
        .unwrap_or(now);

    let mut due = Vec::new();

    for strategy in strategies {
        let Some(cronexpr_str) = schedule_to_cronexpr(&strategy.schedule) else {
            continue;
        };

        let crontab = match parse_cron_expr(cronexpr_str, "backup", &strategy.name) {
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

        if next_fire > window_start && next_fire <= now {
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
