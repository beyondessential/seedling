//! The slice unit files the runtime has written, and the record of them that
//! decides which it may remove.

use std::{
    collections::{BTreeSet, HashSet},
    path::Path,
};

use snafu::{IntoError, ResultExt};

use super::{InvalidUnitNameSnafu, IoSnafu, SystemdError, validate_unit_name};
use crate::system::{confined, types::SliceSpec};

/// What a pass over the slice unit files did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct SliceFileChanges {
    /// The slices whose unit file was written, which need their weights pushed
    /// to the live group.
    pub written: Vec<String>,
    /// Whether a unit file was removed.
    pub removed: bool,
}

/// The slice unit files recorded as written by the runtime.
///
/// A record that cannot be read is an error: reading it as empty would
/// forget every slice the runtime had written, and reading it as complete
/// would let anything in the directory be removed.
pub(super) async fn read_record(record: &Path) -> Result<BTreeSet<String>, SystemdError> {
    match tokio::fs::read_to_string(record).await {
        Ok(text) => Ok(text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(e) => Err(IoSnafu.into_error(e)),
    }
}

async fn write_record(record: &Path, names: &BTreeSet<String>) -> Result<(), SystemdError> {
    let mut text = String::new();
    for name in names {
        text.push_str(name);
        text.push('\n');
    }
    let temp = record.with_extension("tmp");
    tokio::fs::write(&temp, text.as_bytes())
        .await
        .context(IoSnafu)?;
    tokio::fs::rename(&temp, record).await.context(IoSnafu)
}

/// Bring the unit files in `unit_dir` into line with `desired`.
///
/// A slice is recorded before its file is written, so a crash between the two
/// leaves a record of a file that is not there and never a file the record
/// does not name. Only recorded files are removed, and only when `prune` is
/// set: it is not when the caller could not account for every app.
// spec: LINUX#resource-control-groups
pub(super) async fn sync_files(
    unit_dir: &Path,
    record: &Path,
    desired: &[SliceSpec],
    prune: bool,
    content: impl Fn(&SliceSpec) -> String,
) -> Result<SliceFileChanges, SystemdError> {
    let mut recorded = read_record(record).await?;
    let mut changes = SliceFileChanges::default();

    for spec in desired {
        let wanted = content(spec);
        let current = tokio::fs::read_to_string(unit_dir.join(&spec.name))
            .await
            .ok();
        let differs = current.as_deref() != Some(wanted.as_str());
        if recorded.insert(spec.name.clone()) {
            write_record(record, &recorded).await?;
        }
        if differs {
            confined::write_async(unit_dir, &spec.name, wanted.as_bytes())
                .await
                .map_err(|e| match e {
                    confined::ConfinedWriteError::Io { source, .. } => IoSnafu.into_error(source),
                    confined::ConfinedWriteError::Escape { .. } => InvalidUnitNameSnafu {
                        name: spec.name.clone(),
                    }
                    .build(),
                })?;
            changes.written.push(spec.name.clone());
        }
    }

    if prune {
        let wanted: HashSet<&str> = desired.iter().map(|s| s.name.as_str()).collect();
        let mut forgotten = false;
        for name in recorded.clone() {
            if wanted.contains(name.as_str()) {
                continue;
            }
            if validate_unit_name(&name).is_err() || !super::SystemdManager::is_owned_slice(&name) {
                tracing::warn!(%name, "systemd: ignoring a recorded slice with a name no slice has");
                recorded.remove(&name);
                forgotten = true;
                continue;
            }
            match tokio::fs::remove_file(unit_dir.join(&name)).await {
                Ok(()) => changes.removed = true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(IoSnafu.into_error(e)),
            }
            recorded.remove(&name);
            forgotten = true;
        }
        if forgotten {
            write_record(record, &recorded).await?;
        }
    }

    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str) -> SliceSpec {
        SliceSpec {
            name: name.to_owned(),
            description: name.to_owned(),
            weight: 100,
        }
    }

    fn content(spec: &SliceSpec) -> String {
        format!("weight={}\n", spec.weight)
    }

    // spec: LINUX#resource-control-groups
    #[tokio::test]
    async fn only_recorded_slice_files_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let units = dir.path().join("units");
        std::fs::create_dir(&units).unwrap();
        let record = dir.path().join("slice-units");
        // A slice file an operator wrote, shaped like the runtime's.
        std::fs::write(units.join("seedling-operator.slice"), "mine").unwrap();

        let both = [spec("seedling-a.slice"), spec("seedling-b.slice")];
        let first = sync_files(&units, &record, &both, true, content)
            .await
            .unwrap();
        assert_eq!(first.written.len(), 2);
        assert!(!first.removed);

        let only_a = [spec("seedling-a.slice")];
        let second = sync_files(&units, &record, &only_a, true, content)
            .await
            .unwrap();
        assert!(second.removed);
        assert!(units.join("seedling-a.slice").exists());
        assert!(!units.join("seedling-b.slice").exists());
        assert!(
            units.join("seedling-operator.slice").exists(),
            "a file the runtime never wrote is left alone"
        );
        assert_eq!(
            read_record(&record).await.unwrap(),
            BTreeSet::from(["seedling-a.slice".to_owned()])
        );
    }

    // spec: LINUX#resource-control-groups
    #[tokio::test]
    async fn the_record_survives_a_restart_and_nothing_is_removed_without_prune() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("slice-units");
        sync_files(
            dir.path(),
            &record,
            &[spec("seedling-a.slice")],
            true,
            content,
        )
        .await
        .unwrap();

        let held = sync_files(dir.path(), &record, &[], false, content)
            .await
            .unwrap();
        assert!(!held.removed);
        assert!(dir.path().join("seedling-a.slice").exists());

        let pruned = sync_files(dir.path(), &record, &[], true, content)
            .await
            .unwrap();
        assert!(pruned.removed);
        assert!(!dir.path().join("seedling-a.slice").exists());
    }

    // spec: LINUX#resource-control-groups
    #[tokio::test]
    async fn an_unreadable_record_is_an_error_not_an_empty_one() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the record should be a file cannot be read as one.
        let record = dir.path().join("slice-units");
        std::fs::create_dir(&record).unwrap();
        assert!(read_record(&record).await.is_err());
        assert!(
            sync_files(dir.path(), &record, &[], true, content)
                .await
                .is_err()
        );
    }
}
