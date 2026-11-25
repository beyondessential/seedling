use super::blob_forms;

// spec: SEC#redaction
#[test]
fn a_blob_is_withheld_in_each_text_encoding() {
    let forms = blob_forms(b"\x00a~");
    for form in [&b"00617e"[..], b"AGF+", b"AGF-", b"%00a~"] {
        assert!(forms.iter().any(|f| f == form), "{form:?}");
    }
}
