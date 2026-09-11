//! Shared fixtures for the TLS test modules.

/// A self-signed certificate carrying exactly `sans`, PEM-encoded.
///
/// Resolution and supersession both decide what a row serves by parsing its
/// certificate, so a fixture row needs a certificate that really carries the
/// names the test is about: a placeholder PEM makes the row unservable for
/// anything, and un-retirable besides. Tests that want a row claiming a name
/// its certificate does not carry build that mismatch explicitly, by passing a
/// label and `sans` that disagree.
pub fn self_signed_pem(sans: &[&str]) -> String {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("keypair");
    let mut params =
        rcgen::CertificateParams::new(sans.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
            .expect("params");
    params.distinguished_name = rcgen::DistinguishedName::new();
    params.self_signed(&key).expect("self-sign").pem()
}
