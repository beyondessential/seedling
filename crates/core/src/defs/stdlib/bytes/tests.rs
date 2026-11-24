use bytes::Bytes;

use super::*;

// spec: ENC#directory-encoding
#[test]
fn a_directory_encodes_each_file_by_relative_path_in_bytewise_order() {
    let dir = Directory {
        path: "conf".into(),
        files: vec![
            ("b".into(), Bytes::from_static(b"2")),
            ("a/x".into(), Bytes::from_static(b"1")),
        ],
    };
    assert_eq!(
        directory_encoding(&dir).unwrap(),
        [
            &[0, 0, 0, 3][..],
            b"a/x",
            &[0, 0, 0, 0, 0, 0, 0, 1],
            b"1",
            &[0, 0, 0, 1],
            b"b",
            &[0, 0, 0, 0, 0, 0, 0, 1],
            b"2",
        ]
        .concat(),
    );
}

// spec: ENC#byte-arguments
#[test]
fn a_bytes_argument_takes_strings_blobs_and_bundle_files() {
    assert_eq!(
        take_byte_arg("d", "é".into()).unwrap().bytes,
        "é".as_bytes()
    );
    assert_eq!(
        take_byte_arg("d", Dynamic::from_blob(vec![0, 255]))
            .unwrap()
            .bytes,
        [0, 255][..]
    );
    let file = take_byte_arg(
        "d",
        Dynamic::from(File {
            path: "f".into(),
            contents: Bytes::from_static(b"hi"),
        }),
    )
    .unwrap();
    assert_eq!(file.bytes, b"hi"[..]);
    assert!(file.withholdable().is_none());
    let err = take_byte_arg("d", Dynamic::from(1_i64)).err().unwrap();
    assert!(
        err.to_string()
            .contains("`d` must be a string, Blob, File or Directory"),
        "{err}"
    );
}
