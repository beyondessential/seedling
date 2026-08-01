use super::*;
use crate::runtime::db::Db;

fn app(s: &str) -> AppName {
    AppName::new(s).unwrap()
}

fn subject(instance: &str) -> RestartSubject {
    RestartSubject {
        app: app("myapp"),
        instance_id: instance.to_owned(),
        resource_type: Some("deployment".to_owned()),
        resource_name: Some("web".to_owned()),
        generation: Some(3),
    }
}

fn exited(code: i32) -> Option<ExitStatus> {
    Some(ExitStatus {
        kind: ExitKind::Exited,
        code,
    })
}

// r[verify autonomous.restart.record]
// i[verify restart.record]
#[test]
fn records_carry_identity_exit_and_initiator() {
    let db = Db::open_in_memory().expect("open");
    let now = now_ms();
    record(&db, &subject("aa"), Initiator::Supervisor, exited(137), now).expect("record");

    let rows = list(&db, None, None, 10).expect("list");
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.app, "myapp");
    assert_eq!(r.instance_id, "aa");
    assert_eq!(r.resource_type.as_deref(), Some("deployment"));
    assert_eq!(r.resource_name.as_deref(), Some("web"));
    assert_eq!(r.generation, Some(3));
    assert_eq!(r.initiator, Initiator::Supervisor);
    assert_eq!(r.exit_code, Some(137));
    assert_eq!(r.exit_kind, Some(ExitKind::Exited));
}

// i[verify restart.list]
#[test]
fn list_is_most_recent_first_and_filters() {
    let db = Db::open_in_memory().expect("open");
    let now = now_ms();
    record(&db, &subject("aa"), Initiator::Supervisor, None, now - 2000).expect("record");
    record(&db, &subject("aa"), Initiator::Supervisor, None, now - 1000).expect("record");
    record(&db, &subject("bb"), Initiator::Runtime, None, now).expect("record");

    let all = list(&db, None, None, 10).expect("list");
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].instance_id, "bb");

    let only_aa = list(&db, None, Some("aa"), 10).expect("list");
    assert_eq!(only_aa.len(), 2);

    let other_app = list(&db, Some(&app("elsewhere")), None, 10).expect("list");
    assert!(other_app.is_empty());

    let limited = list(&db, None, None, 1).expect("list");
    assert_eq!(limited.len(), 1);
}

// r[verify autonomous.restart.rate]
#[test]
fn runtime_initiated_restarts_are_excluded_from_the_rate() {
    let db = Db::open_in_memory().expect("open");
    let now = now_ms();
    for i in 0..4 {
        record(&db, &subject("aa"), Initiator::Runtime, None, now - i * 100).expect("record");
    }
    assert_eq!(recent_supervisor_count(&db, "aa", 1800).expect("count"), 0);

    record(&db, &subject("aa"), Initiator::Supervisor, None, now).expect("record");
    assert_eq!(recent_supervisor_count(&db, "aa", 1800).expect("count"), 1);
}

// r[verify autonomous.restart.rate]
#[test]
fn restarts_outside_the_window_do_not_count() {
    let db = Db::open_in_memory().expect("open");
    let now = now_ms();
    record(
        &db,
        &subject("aa"),
        Initiator::Supervisor,
        None,
        now - 3_600_000,
    )
    .expect("record");
    record(&db, &subject("aa"), Initiator::Supervisor, None, now).expect("record");

    assert_eq!(recent_supervisor_count(&db, "aa", 1800).expect("count"), 1);
    assert_eq!(recent_supervisor_count(&db, "aa", 7200).expect("count"), 2);
}

// r[verify gc.restarts]
#[test]
fn per_instance_cap_holds_under_a_sustained_crash_loop() {
    let db = Db::open_in_memory().expect("open");
    let now = now_ms();
    for i in 0..(RETAIN_PER_INSTANCE as i64 * 3) {
        record(
            &db,
            &subject("aa"),
            Initiator::Supervisor,
            exited(1),
            now + i,
        )
        .expect("record");
    }
    // A second instance's history must not be pruned by the first's churn.
    record(&db, &subject("bb"), Initiator::Supervisor, None, now).expect("record");

    let kept = list(&db, None, Some("aa"), 1000).expect("list");
    assert_eq!(kept.len(), RETAIN_PER_INSTANCE);
    // The cap keeps the most recent records, which are the diagnostic ones.
    assert_eq!(kept[0].timestamp.as_millisecond(), now + 149);
    assert_eq!(list(&db, None, Some("bb"), 1000).expect("list").len(), 1);
}

#[test]
fn summary_is_absent_until_there_is_history() {
    let db = Db::open_in_memory().expect("open");
    let settings = RestartSettings {
        threshold: 5,
        window_secs: 1800,
    };
    assert!(summary(&db, "aa", settings).expect("summary").is_none());

    let now = now_ms();
    record(&db, &subject("aa"), Initiator::Runtime, None, now - 1000).expect("record");
    record(&db, &subject("aa"), Initiator::Supervisor, exited(2), now).expect("record");

    let s = summary(&db, "aa", settings)
        .expect("summary")
        .expect("some");
    assert_eq!(s.total, 2);
    assert_eq!(s.recent, 1);
    assert_eq!(s.window_secs, 1800);
    assert_eq!(s.last_exit_code, Some(2));
    assert_eq!(s.last_exit_kind, Some(ExitKind::Exited));
    assert!(s.last_at.is_some());
}

#[test]
fn baselines_round_trip_and_clear() {
    let db = Db::open_in_memory().expect("open");
    assert_eq!(baseline(&db, "aa").expect("baseline"), None);
    set_baseline(&db, "aa", 4).expect("set");
    assert_eq!(baseline(&db, "aa").expect("baseline"), Some(4));
    set_baseline(&db, "aa", 0).expect("set");
    assert_eq!(baseline(&db, "aa").expect("baseline"), Some(0));
    clear_baseline(&db, "aa").expect("clear");
    assert_eq!(baseline(&db, "aa").expect("baseline"), None);
}

// r[verify autonomous.restart.rate.settings]
// i[verify restart.settings]
#[test]
fn settings_default_and_update_partially() {
    let db = Db::open_in_memory().expect("open");
    let s = settings(&db).expect("settings");
    assert_eq!(s.threshold, 5);
    assert_eq!(s.window_secs, 1800);

    let s = set_settings(&db, Some(3), None).expect("set");
    assert_eq!(s.threshold, 3);
    assert_eq!(s.window_secs, 1800);
    assert_eq!(settings(&db).expect("settings"), s);

    let s = set_settings(&db, None, Some(600)).expect("set");
    assert_eq!(s.threshold, 3);
    assert_eq!(s.window_secs, 600);
}

// i[verify restart.settings]
#[test]
fn settings_reject_out_of_bounds_values() {
    let db = Db::open_in_memory().expect("open");
    assert_eq!(
        set_settings(&db, Some(1), None),
        Err(SettingsError::ThresholdTooLow)
    );
    assert_eq!(
        set_settings(&db, None, Some(30)),
        Err(SettingsError::WindowTooShort)
    );
    // A rejected update leaves the stored settings untouched.
    let s = settings(&db).expect("settings");
    assert_eq!(s.threshold, 5);
    assert_eq!(s.window_secs, 1800);
}
