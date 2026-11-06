//! The apps reported to Canopy as applications.

use std::collections::BTreeMap;

use seedling_protocol::names::AppName;
use serde_json::{Value, json};

use super::{Result_, check, store};
use crate::runtime::{apps::AppStatus, db::Db, faults};

/// What grading an app's status gave.
#[derive(Debug, PartialEq, Eq)]
struct Graded {
    result: Result_,
    /// Whether this is a grade to keep for an operation that follows.
    record: bool,
}

// r[impl canopy.app.health]
/// Grade an app's `health/app` check from its status.
///
/// A lifecycle operation neither raises nor recovers health, and Canopy closes
/// a check's issue when the check is skipped, so an operating app re-sends the
/// result it had before the operation began.
fn grade(status: &AppStatus, before_operation: Option<Result_>) -> Graded {
    let graded = |result| Graded {
        result,
        record: true,
    };
    match status {
        AppStatus::Running => graded(Result_::Passed),
        AppStatus::Degraded => graded(Result_::Warning),
        AppStatus::Faulted => graded(Result_::Failed),
        AppStatus::Operating { .. } => Graded {
            result: before_operation.unwrap_or(Result_::Skipped),
            record: false,
        },
        AppStatus::NotInstalled | AppStatus::Installing | AppStatus::Uninstalling => Graded {
            result: Result_::Skipped,
            record: false,
        },
    }
}

fn fault_detail(fault: &faults::FaultRecord) -> Value {
    let mut detail = json!({ "kind": fault.kind, "description": fault.description });
    let resource = match (&fault.resource_type, &fault.resource_name) {
        (Some(kind), Some(name)) => Some(format!("{kind}/{name}")),
        (None, Some(name)) => Some(name.clone()),
        _ => None,
    };
    if let (Some(map), Some(resource)) = (detail.as_object_mut(), resource) {
        map.insert("resource".into(), Value::String(resource));
    }
    detail
}

// r[impl canopy.app.key]
/// The `applications` section of a report: one entry per registered app that
/// declares a Canopy type, keyed by the app's name.
///
/// Records each grade that a following operation will re-send. An app that is
/// in the type table but not in `statuses` is not reported: it is not
/// registered, so it has no status to report.
pub(super) fn report_applications(
    db: &Db,
    statuses: &[(AppName, AppStatus)],
) -> rusqlite::Result<Value> {
    let mut applications = BTreeMap::new();
    for app in store::list_apps(db)? {
        let Some((name, status)) = statuses.iter().find(|(n, _)| n.as_str() == app.app) else {
            continue;
        };
        let graded = grade(
            status,
            app.last_grade.as_deref().and_then(Result_::from_stored),
        );
        if graded.record {
            store::set_last_grade(db, &app.app, graded.result.as_str())?;
        }

        let active: Vec<Value> = faults::list_active_faults(db, Some(name))?
            .iter()
            .map(fault_detail)
            .collect();
        let health = check(
            "health/app",
            graded.result,
            json!({ "status": status.name(), "faults": active }),
        );
        applications.insert(
            app.app,
            json!({ "type": app.canopy_type, "health": [health], "detail": {} }),
        );
    }
    Ok(json!(applications))
}

#[cfg(test)]
mod tests;
