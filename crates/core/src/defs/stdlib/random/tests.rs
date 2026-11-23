use super::*;

/// The seed the known answers below were computed under: the bytes 0 to 31.
fn seed() -> Seed {
    Seed::from_bytes(std::array::from_fn(|i| i as u8))
}

// The known answers come from an implementation of the derivation written
// apart from this one, in Python with the standard `hmac` module and the
// `cryptography` package's ChaCha20 (whose 16-byte nonce is the 32-bit
// little-endian block counter followed by the 96-bit nonce):
//
//     def s(x): b = x.encode(); return struct.pack('>I', len(b)) + b
//     def i(n): return struct.pack('>Q', n)
//     def stream(seed, enc, n):
//         key = hmac.new(seed, enc, hashlib.sha256).digest()
//         c = Cipher(algorithms.ChaCha20(key, b'\0' * 16), mode=None).encryptor()
//         return c.update(b'\0' * n)

// spec: RND#byte-stream
/// The crate's nonce and counter layout is RFC 8439's: the keystream from
/// block 1 under the §2.4.2 key and nonce is the RFC's.
#[test]
fn chacha20_matches_rfc_8439() {
    let key: [u8; 32] = std::array::from_fn(|i| i as u8);
    let nonce = [0, 0, 0, 0, 0, 0, 0, 0x4a, 0, 0, 0, 0];
    let mut stream = ChaCha20::new(&key.into(), &nonce.into());
    let mut block0 = [0u8; 64];
    take_bytes(&mut stream, &mut block0);
    let mut out = [0u8; 16];
    take_bytes(&mut stream, &mut out);
    assert_eq!(hex(&out), "224f51f3401bd9e12fde276fb8631ded");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// spec: RND#call-encoding
#[test]
fn a_string_field_is_its_length_then_its_utf8() {
    assert_eq!(
        encode(&[Field::Str("αb"), Field::Int(258)]).unwrap(),
        [0, 0, 0, 3, 0xce, 0xb1, b'b', 0, 0, 0, 0, 0, 0, 1, 2],
    );
}

// spec: RND#values
#[test]
fn random_bytes_known_answer() {
    let bytes = random_bytes(&seed(), "db-password", 80).unwrap();
    assert_eq!(
        hex(&bytes),
        "1e307397e271a3366619800bb29296f676c4b6da126435366566039ba9645f43\
         3d903d919c496c8431e42cfa5f669923d5a9e3b22ddd27b8a445bdb29a7a318d\
         a02fe3fa188655a752f671b44759a132",
    );
}

// spec: RND#values
#[test]
fn random_string_known_answers() {
    let alphanumeric: Vec<char> = "abcdefghijklmnopqrstuvwxyz0123456789".chars().collect();
    assert_eq!(
        random_string(&seed(), "token", 24, &alphanumeric).unwrap(),
        "g95xoi4ttyiu7myaytmc33sx",
    );
    let greek: Vec<char> = "αβγδ".chars().collect();
    assert_eq!(
        random_string(&seed(), "greek", 10, &greek).unwrap(),
        "βγδββγβββγ"
    );
}

// spec: RND#values
#[test]
fn uuid_v4_known_answer() {
    assert_eq!(
        uuid_v4(&seed(), "instance").unwrap(),
        "a4555b20-bf11-416d-a1be-f82adabd9e22"
    );
}

// spec: RND#values
#[test]
fn a_word_in_the_biased_top_is_discarded() {
    // 2^32 mod 3 = 1, so only the very last word is discarded.
    assert_eq!(
        pick(u32::MAX - 1, 3),
        Some(((u64::from(u32::MAX) - 1) % 3) as usize)
    );
    assert_eq!(pick(u32::MAX, 3), None);
    // 2^32 mod 10 = 6: the top six words are discarded.
    assert_eq!(
        pick(u32::MAX - 6, 10),
        Some(((u64::from(u32::MAX) - 6) % 10) as usize)
    );
    assert_eq!(pick(u32::MAX - 5, 10), None);
    // A power of two divides 2^32, so nothing is discarded.
    assert_eq!(pick(u32::MAX, 4), Some(3));
}

// spec: RND#values
#[test]
fn uuid_v4_carries_the_version_and_variant() {
    for label in ["a", "b", "c", "d"] {
        let uuid = uuid::Uuid::parse_str(&uuid_v4(&seed(), label).unwrap()).unwrap();
        assert_eq!(uuid.get_version_num(), 4);
        assert_eq!(uuid.get_variant(), uuid::Variant::RFC4122);
    }
}

// spec: RND#seeds
#[test]
fn values_differ_by_seed_label_and_argument() {
    let other = Seed::from_bytes([7; 32]);
    let base = random_bytes(&seed(), "x", 16).unwrap();
    assert_ne!(base, random_bytes(&other, "x", 16).unwrap());
    assert_ne!(base, random_bytes(&seed(), "y", 16).unwrap());
    assert_ne!(base[..], random_bytes(&seed(), "x", 17).unwrap()[..16]);
}
