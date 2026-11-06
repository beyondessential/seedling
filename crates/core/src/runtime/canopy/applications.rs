//! The apps reported to Canopy as applications.

use std::collections::{BTreeMap, HashMap, HashSet};

use seedling_protocol::names::AppName;
use serde_json::{Value, json};

use super::{Result_, check, store};
use crate::runtime::{apps::AppStatus, db::Db, faults};

/// What grading an app's status gave.
#[derive(Debug, PartialEq, Eq)]
struct Graded {
    result: Result_,
    hold: Hold,
}

/// What becomes of the grade held against the app for an operation to re-send.
#[derive(Debug, PartialEq, Eq)]
enum Hold {
    /// Hold this result: the app was graded in its own right.
    Graded,
    /// Leave the held result alone, since this one is not the app's own.
    Unchanged,
    /// Drop the held result. An app that has finished uninstalling starts its
    /// next install ungraded, so the grade from the last one is not its to
    /// re-send.
    Dropped,
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
        hold: Hold::Graded,
    };
    let skipped = |hold| Graded {
        result: Result_::Skipped,
        hold,
    };
    match status {
        AppStatus::Running => graded(Result_::Passed),
        AppStatus::Degraded => graded(Result_::Warning),
        AppStatus::Faulted => graded(Result_::Failed),
        AppStatus::Operating { .. } => Graded {
            result: before_operation.unwrap_or(Result_::Skipped),
            hold: Hold::Unchanged,
        },
        // Held until the uninstall finishes, so an operation during one still
        // re-sends the result the app had going into it.
        AppStatus::Installing | AppStatus::Uninstalling => skipped(Hold::Unchanged),
        AppStatus::NotInstalled => skipped(Hold::Dropped),
    }
}

fn fault_detail(fault: &faults::FaultRecord) -> Value {
    let mut detail = json!({ "kind": fault.kind, "description": fault.description });
    // A fault carries its type and its name independently, so either alone is
    // a shape that reaches here. Say what there is: a fault against a kind of
    // thing still tells an operator more than no resource at all.
    let resource = match (&fault.resource_type, &fault.resource_name) {
        (Some(kind), Some(name)) => Some(format!("{kind}/{name}")),
        (Some(kind), None) => Some(kind.clone()),
        (None, Some(name)) => Some(name.clone()),
        (None, None) => None,
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
    // The report runs every minute. Looking each app's status up by scanning
    // the list, or asking for its faults in a query of its own, would grow the
    // tick's cost with the app count on top of the work itself, so both are
    // gathered once and only for the apps that will be reported.
    let by_name: HashMap<&str, &AppStatus> = statuses
        .iter()
        .map(|(name, status)| (name.as_str(), status))
        .collect();

    let reported: Vec<(store::CanopyApp, &AppStatus)> = store::list_apps(db)?
        .into_iter()
        .filter_map(|app| {
            let status = by_name.get(app.app.as_str()).copied()?;
            Some((app, status))
        })
        .collect();

    let reporting: HashSet<&str> = reported.iter().map(|(app, _)| app.app.as_str()).collect();
    let mut faults_by_app: HashMap<&str, Vec<Value>> = HashMap::new();
    let active = faults::list_active_faults(db, None)?;
    for fault in &active {
        let app = fault.app.as_str();
        if reporting.contains(app) {
            faults_by_app
                .entry(app)
                .or_default()
                .push(fault_detail(fault));
        }
    }

    let mut applications = BTreeMap::new();
    for (app, status) in reported {
        let held = app.last_grade.as_deref();
        let graded = grade(status, held.and_then(Result_::from_stored));
        // Only when it differs: the grade is the same from one minute to the
        // next for as long as nothing happens to the app, and a write that
        // stores what is already there costs a WAL write for nothing.
        match graded.hold {
            Hold::Graded if held != Some(graded.result.as_str()) => {
                store::set_last_grade(db, &app.app, Some(graded.result.as_str()))?;
            }
            Hold::Dropped if held.is_some() => store::set_last_grade(db, &app.app, None)?,
            _ => {}
        }

        let health = check(
            "health/app",
            graded.result,
            json!({
                "status": status.name(),
                "faults": faults_by_app.remove(app.app.as_str()).unwrap_or_default(),
            }),
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
