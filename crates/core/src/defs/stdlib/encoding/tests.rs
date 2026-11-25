use rhai::{Dynamic, Engine, EvalAltResult};

use super::super::{HelperLimits, module};

// The known answers are the published test vectors of each standard, checked
// against Python's `hashlib`, `hmac`, `base64` and `uuid` modules and the
// BLAKE3 team's `b3sum`.

fn engine() -> Engine {
    let mut engine = Engine::new();
    engine.register_static_module("std", module().into());
    engine.set_default_tag(Dynamic::from(HelperLimits {
        derived_values: 0,
        password_hashes: 0,
    }));
    engine
}

fn eval(src: &str) -> Result<Dynamic, Box<EvalAltResult>> {
    engine().eval::<Dynamic>(src)
}

fn text(src: &str) -> String {
    eval(src).unwrap().into_string().unwrap()
}

fn throws(src: &str) -> String {
    eval(src).expect_err(src).to_string()
}

/// The lowercase hex of a script expression's Blob.
fn digest(src: &str) -> String {
    hex::encode(eval(src).unwrap().into_blob().unwrap())
}

// spec: ENC#encodings
#[test]
fn the_encoders_match_rfc_4648() {
    let cases = [
        ("", "", "", ""),
        ("f", "66", "Zg==", "Zg"),
        ("fo", "666f", "Zm8=", "Zm8"),
        ("foo", "666f6f", "Zm9v", "Zm9v"),
        ("foob", "666f6f62", "Zm9vYg==", "Zm9vYg"),
        ("fooba", "666f6f6261", "Zm9vYmE=", "Zm9vYmE"),
        ("foobar", "666f6f626172", "Zm9vYmFy", "Zm9vYmFy"),
    ];
    for (input, base16, base64, base64url) in cases {
        assert_eq!(text(&format!("std::encode_hex({input:?})")), base16);
        assert_eq!(text(&format!("std::encode_base64({input:?})")), base64);
        assert_eq!(
            text(&format!("std::encode_base64url({input:?})")),
            base64url
        );
        for (decoder, encoded) in [
            ("decode_hex", base16),
            ("decode_base64", base64),
            ("decode_base64url", base64url),
        ] {
            let decoded = eval(&format!("std::{decoder}({encoded:?})"))
                .unwrap()
                .into_blob()
                .unwrap();
            assert_eq!(decoded, input.as_bytes(), "{decoder}({encoded:?})");
        }
    }
}

// spec: ENC#encodings
#[test]
fn base64url_uses_its_own_alphabet() {
    assert_eq!(
        text("std::encode_base64(std::decode_hex(\"fbff\"))"),
        "+/8="
    );
    assert_eq!(
        text("std::encode_base64url(std::decode_hex(\"fbff\"))"),
        "-_8"
    );
}

// spec: ENC#encodings
#[test]
fn decode_hex_takes_either_case() {
    assert_eq!(text("std::encode_hex(std::decode_hex(\"aBcD\"))"), "abcd");
}

// spec: ENC#encodings
#[test]
fn the_decoders_are_strict() {
    for src in [
        r#"std::decode_hex("abc")"#,
        r#"std::decode_hex("zz")"#,
        r#"std::decode_hex(" ab")"#,
        r#"std::decode_base64("Zg")"#,
        r#"std::decode_base64("Zg=")"#,
        r#"std::decode_base64("Zm9v YmFy")"#,
        r#"std::decode_base64("Zh==")"#,
        r#"std::decode_base64("-_8=")"#,
        r#"std::decode_base64url("Zg==")"#,
        r#"std::decode_base64url("Z")"#,
        r#"std::decode_base64url("Zh")"#,
        r#"std::decode_base64url("+/8")"#,
        r#"std::decode_utf8(std::decode_hex("c328"))"#,
    ] {
        throws(src);
    }
}

// spec: SEC#redaction
#[test]
fn errors_leave_out_the_argument() {
    for (src, held) in [
        (r#"std::decode_hex("aQ")"#, &["Q"][..]),
        (r#"std::decode_base64("Zm9v~mFy")"#, &["~", "126"]),
        (r#"std::decode_base64url("Zm9v~mFy")"#, &["~", "126"]),
        (r#"std::decode_base64("Zh==")"#, &["'h'", "0x68"]),
        (r#"std::uuid_v5("s3cr3t", "x")"#, &["s3cr3t"]),
        (r#"std::decode_percent("s3cr3t%")"#, &["s3cr3t"]),
    ] {
        let err = throws(src);
        assert!(held.iter().all(|h| !err.contains(h)), "{err}");
    }
}

// spec: ENC#encodings
#[test]
fn the_utf8_helpers_round_trip_and_keep_empty_empty() {
    assert_eq!(text(r#"std::decode_utf8(std::encode_utf8("ā€"))"#), "ā€");
    assert_eq!(digest(r#"std::encode_utf8("é")"#), "c3a9");
    assert_eq!(digest(r#"std::encode_utf8("")"#), "");
    assert_eq!(text(r#"std::decode_utf8("")"#), "");
}

// spec: ENC#encodings
#[test]
fn encode_percent_keeps_only_unreserved_characters() {
    let unreserved = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    assert_eq!(
        text(&format!("std::encode_percent({unreserved:?})")),
        unreserved
    );
    for b in (0..=0x7f_u8).filter(|b| !unreserved.as_bytes().contains(b)) {
        assert_eq!(
            text(&format!(
                r#"std::encode_percent(std::decode_hex("{b:02x}"))"#
            )),
            format!("%{b:02X}")
        );
    }
    assert_eq!(text(r#"std::encode_percent("a b/é")"#), "a%20b%2F%C3%A9");
    assert_eq!(text(r#"std::encode_percent("")"#), "");
}

// spec: ENC#encodings
#[test]
fn decode_percent_reads_a_url_component_as_carried() {
    assert_eq!(digest(r#"std::decode_percent("%2f%2F")"#), "2f2f");
    assert_eq!(
        text(r#"std::decode_utf8(std::decode_percent("a+b=c&d:e!f%20g"))"#),
        "a+b=c&d:e!f g"
    );
    assert_eq!(
        text(r#"std::decode_utf8(std::decode_percent("é%C3%A9"))"#),
        "éé"
    );
    assert!(eval(r#"std::decode_percent("")"#).unwrap().is_blob());
    assert_eq!(digest(r#"std::decode_percent("")"#), "");
}

// spec: ENC#encodings
#[test]
fn decode_percent_throws_on_a_malformed_escape() {
    for (src, at) in [("%", 0), ("%4", 0), ("ab%zz", 2), ("%41%", 3), ("%4g", 0)] {
        let err = throws(&format!("std::decode_percent({src:?})"));
        assert!(err.contains(&format!("position {at}")), "{err}");
    }
}

// spec: ENC#encodings
#[test]
fn percent_encoding_round_trips_every_byte() {
    let all: String = (0..=0xff_u8).map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        digest(&format!(
            r#"std::decode_percent(std::encode_percent(std::decode_hex("{all}")))"#
        )),
        all
    );
}

// spec: ENC#digests
#[test]
fn the_digests_match_their_standards() {
    assert_eq!(
        digest(r#"std::digest_sha256("abc")"#),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        digest(r#"std::digest_sha512("abc")"#),
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
         2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    );
    assert_eq!(
        digest(r#"std::digest_blake2b("abc")"#),
        "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d1\
         7d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923"
    );
    assert_eq!(
        digest(r#"std::digest_blake3("")"#),
        "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    );
    assert_eq!(
        digest(r#"std::digest_blake3("abc")"#),
        "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
    );
}

// spec: ENC#digests
#[test]
fn a_digest_of_a_string_is_the_digest_of_its_utf8() {
    assert_eq!(
        digest(r#"std::digest_sha256("é")"#),
        digest(r#"std::digest_sha256(std::decode_hex("c3a9"))"#),
    );
}

// spec: ENC#message-authentication
#[test]
fn the_hmacs_match_rfc_4231() {
    assert_eq!(
        digest(r#"std::hmac_sha256("Jefe", "what do ya want for nothing?")"#),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    assert_eq!(
        digest(r#"std::hmac_sha512("Jefe", "what do ya want for nothing?")"#),
        "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea250554\
         9758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737"
    );
    let long_key = "aa".repeat(131);
    assert_eq!(
        digest(&format!(
            r#"std::hmac_sha256(std::decode_hex("{long_key}"), "Test Using Larger Than Block-Size Key - Hash Key First")"#
        )),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
}

// spec: ENC#name-based-uuids
#[test]
fn uuid_v5_matches_rfc_9562() {
    assert_eq!(
        text(r#"std::uuid_v5("6ba7b810-9dad-11d1-80b4-00c04fd430c8", "www.example.com")"#),
        "2ed6657d-e927-568b-95e1-2665a8aea6a2"
    );
    assert_eq!(
        text(r#"std::uuid_v5("6BA7B810-9DAD-11D1-80B4-00C04FD430C8", "www.example.com")"#),
        "2ed6657d-e927-568b-95e1-2665a8aea6a2"
    );
}

// spec: ENC#name-based-uuids
#[test]
fn uuid_v5_takes_only_a_hyphenated_namespace() {
    for namespace in [
        "6ba7b8109dad11d180b400c04fd430c8",
        "{6ba7b810-9dad-11d1-80b4-00c04fd430c8}",
        "urn:uuid:6ba7b810-9dad-11d1-80b4-00c04fd430c8",
        "6ba7b810-9dad-11d1-80b4-00c04fd430cg",
        "6ba7b8109-dad-11d1-80b4-00c04fd430c8",
    ] {
        let err = throws(&format!("std::uuid_v5({namespace:?}, \"x\")"));
        assert!(err.contains("hyphenated"), "{err}");
    }
}

// spec: ENC#byte-arguments
#[test]
fn a_bytes_argument_of_another_type_throws() {
    let err = throws("std::digest_sha256(1)");
    assert!(
        err.contains("`data` must be a string, Blob, File or Directory"),
        "{err}"
    );
    let err = throws("std::decode_hex(std::encode_utf8(\"ab\"))");
    assert!(err.contains("`text` must be a string"), "{err}");
}

// spec: ENC#results
#[test]
fn a_result_beyond_the_engine_limits_fails() {
    let mut engine = engine();
    engine.set_max_string_size(9);
    engine.set_max_array_size(4);
    let err = engine
        .eval::<Dynamic>(r#"std::encode_hex("hello")"#)
        .unwrap_err();
    assert!(
        matches!(*err, EvalAltResult::ErrorDataTooLarge(..)),
        "{err}"
    );
    let err = engine
        .eval::<Dynamic>(r#"std::encode_utf8("hello")"#)
        .unwrap_err();
    assert!(
        matches!(*err, EvalAltResult::ErrorDataTooLarge(..)),
        "{err}"
    );
    let err = engine
        .eval::<Dynamic>(r#"std::encode_percent("  ab")"#)
        .unwrap_err();
    assert!(
        matches!(*err, EvalAltResult::ErrorDataTooLarge(..)),
        "{err}"
    );
}
