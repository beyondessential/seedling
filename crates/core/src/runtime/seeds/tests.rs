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
        let inner = inner.take_derived(|_| false, 0).unwrap();
        assert!(inner.found_in("inner") && !inner.found_in("outer"));
    }
    assert_eq!(app_seed().unwrap().expose(), outer.expose());
    let outer = _outer.take_derived(|_| false, 0).unwrap();
    assert!(outer.found_in("outer") && !outer.found_in("inner"));
}

// spec: SEC#redaction
#[test]
fn a_form_derived_twice_is_recorded_once() {
    let frame = AppSeedFrame::enter(&AppSeed::for_tests());
    record_derived(["same".to_owned(), "same".to_owned()]);
    record_derived(["same".to_owned()]);
    assert_eq!(frame.take_derived(|_| false, 0).unwrap().len(), 1);
}

// spec: SEC#redaction
#[test]
fn a_form_is_found_wherever_it_sits() {
    let mut forms = Forms::default();
    forms.insert(b"\0secret");
    forms.insert(b"x");
    for text in [&b"\0secret"[..], b"a\0secret", b"\0secretb", b"..x.."] {
        assert!(forms.found_in(text), "{text:?}");
    }
    for text in [&b"secret"[..], b"\0secre", b"\0\0secre", b"", b"yz"] {
        assert!(!forms.found_in(text), "{text:?}");
    }
}

// spec: SEC#redaction
#[test]
fn a_long_form_is_found_in_a_long_text_in_one_pass() {
    let form: Vec<u8> = (0..1 << 19).map(|i: u32| (i * 7 % 251) as u8).collect();
    let mut text = vec![b'.'; 1 << 19];
    text.extend(&form);
    let mut forms = Forms::default();
    forms.insert(&form);
    assert!(forms.found_in(&text));
    text.pop();
    assert!(!forms.found_in(&text));
}
