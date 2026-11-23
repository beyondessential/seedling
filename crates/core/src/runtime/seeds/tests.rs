use super::*;

// spec: SEC#seeds
#[test]
fn a_sealed_seed_opens_to_itself() {
    let cipher = Cipher::for_tests();
    let seed = Seed::draw().unwrap();
    let opened = Seed::open(&cipher, &seed.seal(&cipher).unwrap()).unwrap();
    assert_eq!(opened.expose(), seed.expose());
}

// spec: SEC#app-seeds
#[test]
fn a_seed_sealed_under_another_key_is_unreadable() {
    let sealed = Seed::draw().unwrap().seal(&Cipher::for_tests()).unwrap();
    let other = Cipher::for_tests_with(&[7; 32]);
    assert!(matches!(
        AppSeed::open(&other, &sealed),
        AppSeed::Unreadable(_)
    ));
}

// spec: SEC#seeds
#[test]
fn a_seed_never_prints() {
    let seed = Seed::from_bytes([0xab; SEED_LEN]);
    let printed = format!("{seed:?} {:?}", AppSeed::Ready(seed.clone()));
    assert!(!printed.contains("ab"), "{printed}");
    assert!(!printed.contains("171"), "{printed}");
}

#[test]
fn draws_differ() {
    assert_ne!(
        Seed::draw().unwrap().expose(),
        Seed::draw().unwrap().expose()
    );
}

// spec: SEC#app-seeds
#[test]
fn an_unreadable_seed_is_not_reported_as_missing() {
    let missing = app_seed().unwrap_err();
    let _frame = AppSeedFrame::enter(&AppSeed::Unreadable("bad tag".into()));
    let unreadable = app_seed().unwrap_err();
    assert_ne!(missing, unreadable);
    assert!(unreadable.contains("bad tag"), "{unreadable}");
}

#[test]
fn a_nested_frame_restores_its_callers() {
    let outer = Seed::from_bytes([1; SEED_LEN]);
    let _outer = AppSeedFrame::enter(&AppSeed::Ready(outer.clone()));
    record_derived(["outer".to_owned()]);
    {
        let inner = AppSeedFrame::enter(&AppSeed::Ready(Seed::from_bytes([2; SEED_LEN])));
        record_derived(["inner".to_owned()]);
        assert_eq!(app_seed().unwrap().expose(), &[2; SEED_LEN]);
        assert_eq!(inner.take_derived().iter().collect::<Vec<_>>(), ["inner"]);
    }
    assert_eq!(app_seed().unwrap().expose(), outer.expose());
    assert_eq!(_outer.take_derived().iter().collect::<Vec<_>>(), ["outer"]);
}

// spec: SEC#redaction
#[test]
fn a_form_derived_twice_is_recorded_once() {
    let frame = AppSeedFrame::enter(&AppSeed::for_tests());
    record_derived(["same".to_owned(), "same".to_owned()]);
    record_derived(["same".to_owned()]);
    assert_eq!(frame.take_derived().iter().count(), 1);
}
