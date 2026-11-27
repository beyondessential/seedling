use std::collections::BTreeMap;

use seedling_protocol::names::{ActionName, AppName, GateName};

use super::*;
use crate::runtime::db::Db;
use crate::runtime::secrets::Cipher;

fn app() -> AppName {
    AppName::new_unchecked("demo")
}

fn limits() -> crate::ScriptLimits {
    crate::ScriptLimits::default()
}

fn run_with(definition: &str, value: &str) -> Run {
    let stored = BTreeMap::from([("pattern".to_owned(), value.to_owned())]);
    Run::new(
        definition,
        &stored,
        Some(&crate::runtime::seeds::AppSeed::for_tests()),
        &limits(),
    )
}

fn action() -> Thing {
    Thing::Action(ActionName::new_unchecked("backup"))
}

fn runaway_faults(db: &Db) -> Vec<faults::FaultRecord> {
    faults::list_active_faults_in_scope(
        db,
        &faults::FaultScope::AppKind(app(), EVALUATION_RUNAWAY.to_owned()),
    )
    .unwrap()
}

// spec: FLT#app-faults
#[test]
fn a_subject_names_the_kind_of_thing_as_well_as_its_name() {
    let gate = Thing::Gate(GateName::new_unchecked("backup"));
    assert_ne!(action().subject(), gate.subject());
    for thing in [Thing::Definition, action(), gate] {
        assert_eq!(Thing::from_subject(&thing.subject()), Some(thing));
    }
    assert_eq!(Thing::from_subject("volume:backup"), None);
}

// spec: FLT#withholding-faults
#[test]
fn only_the_definition_and_gate_bodies_withhold() {
    assert!(faults::withholds(EVALUATION_RUNAWAY, ""));
    assert!(faults::withholds(EVALUATION_RUNAWAY, "gate:migrate"));
    assert!(faults::withholds(BREAKER_STATE_UNREADABLE, "gate:migrate"));
    assert!(!faults::withholds(EVALUATION_RUNAWAY, "action:backup"));
    assert!(!faults::withholds(EVALUATION_RUNAWAY, "shell:console"));
}

// spec: LIM#breakers
#[test]
fn a_tripped_breaker_holds_for_its_run_alone() {
    let (db, cipher) = (Db::open_in_memory().unwrap(), Cipher::for_tests());
    let run = run_with("sha256:a", "x+");
    assert_eq!(
        standing(&db, &cipher, &app(), &action(), || Ok(run.clone())),
        Ok(Standing::Clear)
    );

    trip(&db, &cipher, &app(), &action(), &run, 3, &limits()).unwrap();
    assert_eq!(
        standing(&db, &cipher, &app(), &action(), || Ok(run.clone())),
        Ok(Standing::Tripped)
    );
    let other = run_with("sha256:a", "y+");
    assert_eq!(
        standing(&db, &cipher, &app(), &action(), || Ok(other.clone())),
        Ok(Standing::TrippedForOther)
    );
    assert_eq!(runaway_faults(&db).len(), 1);
}

// spec: LIM#breakers
#[test]
fn a_tripped_breaker_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seedling.db");
    let cipher = Cipher::for_tests();
    let run = run_with("sha256:a", "x+");
    {
        let db = Db::open(&path).unwrap();
        trip(&db, &cipher, &app(), &action(), &run, 3, &limits()).unwrap();
    }
    LAST_IN_TIME.lock().clear();
    let db = Db::open(&path).unwrap();
    assert_eq!(
        standing(&db, &cipher, &app(), &action(), || Ok(run.clone())),
        Ok(Standing::Tripped)
    );
}

// spec: LIM#breakers
// spec: FLT#filing-and-clearing
#[test]
fn a_second_runaway_with_other_inputs_records_them_without_clearing_the_fault() {
    let (db, cipher) = (Db::open_in_memory().unwrap(), Cipher::for_tests());
    let first = run_with("sha256:a", "x+");
    let second = run_with("sha256:a", "y+");
    trip(&db, &cipher, &app(), &action(), &first, 3, &limits()).unwrap();
    let id = runaway_faults(&db)[0].id.clone();

    trip(&db, &cipher, &app(), &action(), &second, 3, &limits()).unwrap();
    let active = runaway_faults(&db);
    assert_eq!(active.len(), 1);
    assert_eq!(
        active[0].id, id,
        "the fault was not cleared and filed again"
    );
    assert_eq!(
        standing(&db, &cipher, &app(), &action(), || Ok(second.clone())),
        Ok(Standing::Tripped)
    );
    assert_eq!(
        standing(&db, &cipher, &app(), &action(), || Ok(first.clone())),
        Ok(Standing::TrippedForOther)
    );
}

// spec: LIM#breakers
#[test]
fn an_in_time_run_with_other_inputs_resets_the_breaker() {
    let (db, cipher) = (Db::open_in_memory().unwrap(), Cipher::for_tests());
    let tripped = run_with("sha256:a", "x+");
    trip(&db, &cipher, &app(), &action(), &tripped, 3, &limits()).unwrap();

    let other = run_with("sha256:a", "y+");
    finished_in_time(
        &db,
        &cipher,
        &app(),
        &action(),
        &other,
        Standing::TrippedForOther,
    );
    assert!(runaway_faults(&db).is_empty());
    assert_eq!(
        standing(&db, &cipher, &app(), &action(), || Ok(tripped.clone())),
        Ok(Standing::Clear)
    );
}

// spec: LIM#breakers
#[test]
fn an_in_time_run_of_what_tripped_the_breaker_resets_it() {
    let (db, cipher) = (Db::open_in_memory().unwrap(), Cipher::for_tests());
    let tripped = run_with("sha256:a", "x+");
    trip(&db, &cipher, &app(), &action(), &tripped, 3, &limits()).unwrap();

    // As an update's evaluation does: it ran untracked, so it was not
    // refused, and the runtime adopted the definition it finished in time.
    // The breaker resets without its standing being read at all.
    adopted_in_time(&db, &cipher, &app(), &action(), &tripped);
    assert!(runaway_faults(&db).is_empty());
    assert_eq!(
        standing(&db, &cipher, &app(), &action(), || Ok(tripped.clone())),
        Ok(Standing::Clear)
    );
}

// spec: LIM#breakers
#[test]
fn clearing_the_fault_resets_the_breaker() {
    let (db, cipher) = (Db::open_in_memory().unwrap(), Cipher::for_tests());
    let run = run_with("sha256:a", "x+");
    trip(&db, &cipher, &app(), &action(), &run, 3, &limits()).unwrap();
    faults::clear_all_faults_for_app(&db, &app()).unwrap();
    assert_eq!(
        standing(&db, &cipher, &app(), &action(), || Ok(run.clone())),
        Ok(Standing::Clear)
    );
}

// spec: LIM#breakers
#[test]
fn a_breaker_whose_state_cannot_be_read_is_never_clear() {
    let (db, cipher) = (Db::open_in_memory().unwrap(), Cipher::for_tests());
    let run = run_with("sha256:a", "x+");
    trip(&db, &cipher, &app(), &action(), &run, 3, &limits()).unwrap();
    db.conn.execute("DROP TABLE breakers", []).unwrap();

    assert!(standing(&db, &cipher, &app(), &action(), || Ok(run.clone())).is_err());
    let held = faults::list_active_faults_in_scope(
        &db,
        &faults::FaultScope::AppKind(app(), BREAKER_STATE_UNREADABLE.to_owned()),
    )
    .unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].subject, action().subject());
}

// spec: LIM#breakers
#[test]
fn a_closure_no_longer_declared_has_its_breaker_reset() {
    let (db, cipher) = (Db::open_in_memory().unwrap(), Cipher::for_tests());
    let run = run_with("sha256:a", "x+");
    trip(&db, &cipher, &app(), &action(), &run, 3, &limits()).unwrap();
    reset_undeclared(&db, &app(), &crate::defs::app::AppDef::default()).unwrap();
    assert!(runaway_faults(&db).is_empty());
}

// spec: FLT#app-faults
#[test]
fn the_likely_cause_names_differing_inputs_and_never_their_values() {
    let (db, cipher) = (Db::open_in_memory().unwrap(), Cipher::for_tests());
    let app = AppName::new_unchecked("cause");
    let good = run_with("sha256:a", "secret-good");
    finished_in_time(&db, &cipher, &app, &action(), &good, Standing::Clear);

    let bad = run_with("sha256:a", "secret-bad");
    trip(&db, &cipher, &app, &action(), &bad, 3, &limits()).unwrap();
    let fault = &faults::list_active_faults(&db, Some(&app)).unwrap()[0];
    assert!(
        fault.description.contains("the inputs"),
        "{}",
        fault.description
    );
    assert!(
        fault.description.contains("parameter `pattern`"),
        "{}",
        fault.description
    );
    assert!(
        !fault.description.contains("secret-bad"),
        "{}",
        fault.description
    );
    assert!(
        !fault.description.contains("secret-good"),
        "{}",
        fault.description
    );
}

// spec: FLT#app-faults
#[test]
fn the_likely_cause_is_the_definition_when_only_it_differs() {
    let (db, cipher) = (Db::open_in_memory().unwrap(), Cipher::for_tests());
    let app = AppName::new_unchecked("definition-cause");
    finished_in_time(
        &db,
        &cipher,
        &app,
        &action(),
        &run_with("sha256:a", "x"),
        Standing::Clear,
    );
    trip(
        &db,
        &cipher,
        &app,
        &action(),
        &run_with("sha256:b", "x"),
        3,
        &limits(),
    )
    .unwrap();
    let fault = &faults::list_active_faults(&db, Some(&app)).unwrap()[0];
    assert!(
        fault
            .description
            .contains("the likely cause is the definition"),
        "{}",
        fault.description
    );
}

// spec: FLT#app-faults
#[test]
fn the_likely_cause_is_unknown_without_an_in_time_evaluation() {
    let (db, cipher) = (Db::open_in_memory().unwrap(), Cipher::for_tests());
    let app = AppName::new_unchecked("no-record");
    trip(
        &db,
        &cipher,
        &app,
        &action(),
        &run_with("sha256:a", "x"),
        3,
        &limits(),
    )
    .unwrap();
    let fault = &faults::list_active_faults(&db, Some(&app)).unwrap()[0];
    assert!(
        fault.description.contains("unknown"),
        "{}",
        fault.description
    );
}

// spec: LIM#breakers
#[test]
fn raising_the_limit_is_a_change_of_inputs() {
    let (db, cipher) = (Db::open_in_memory().unwrap(), Cipher::for_tests());
    let stored = BTreeMap::new();
    let seed = crate::runtime::seeds::AppSeed::for_tests();
    let run = Run::new("sha256:a", &stored, Some(&seed), &limits());
    trip(&db, &cipher, &app(), &Thing::Definition, &run, 1, &limits()).unwrap();
    let raised = crate::ScriptLimits {
        max_evaluation_time: 60,
        ..limits()
    };
    let rerun = Run::new("sha256:a", &stored, Some(&seed), &raised);
    assert_eq!(
        standing(&db, &cipher, &app(), &Thing::Definition, || Ok(rerun.clone())),
        Ok(Standing::TrippedForOther)
    );
}

// spec: FLTS#fault-records
#[test]
fn a_closure_fault_names_its_closure_and_a_definition_fault_names_nothing() {
    let (db, cipher) = (Db::open_in_memory().unwrap(), Cipher::for_tests());
    let run = run_with("sha256:a", "x+");
    trip(&db, &cipher, &app(), &action(), &run, 3, &limits()).unwrap();
    trip(&db, &cipher, &app(), &Thing::Definition, &run, 3, &limits()).unwrap();

    let faults = runaway_faults(&db);
    let named = |subject: &str| {
        let fault = faults.iter().find(|f| f.subject == subject).unwrap();
        (
            fault.resource_type.as_deref(),
            fault.resource_name.as_deref(),
            fault.instance_id.as_deref(),
        )
    };
    assert_eq!(
        named("action:backup"),
        (Some("action"), Some("backup"), None)
    );
    assert_eq!(named(""), (None, None, None));
}
