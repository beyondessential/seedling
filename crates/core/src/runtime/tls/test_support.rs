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

/// A certificate carrying exactly `sans`, signed by a separate CA so its
/// issuer differs from its subject.
///
/// Distinct from [`self_signed_pem`] where it matters: resolution and
/// supersession read self-issuance from the certificate rather than from the
/// stored column, so a test about CA-issued-versus-self-signed has to use
/// certificates that really differ, not a flag that says they do.
pub fn ca_signed_pem(sans: &[&str]) -> String {
    let ca_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("ca keypair");
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("ca params");
    ca_params.distinguished_name = rcgen::DistinguishedName::new();
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Test CA");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let issuer = rcgen::Issuer::new(ca_params, ca_key);

    let leaf_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("keypair");
    let mut leaf =
        rcgen::CertificateParams::new(sans.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
            .expect("params");
    leaf.distinguished_name = rcgen::DistinguishedName::new();
    leaf.distinguished_name.push(
        rcgen::DnType::CommonName,
        sans.first().copied().unwrap_or("leaf"),
    );
    leaf.signed_by(&leaf_key, &issuer)
        .expect("ca signs leaf")
        .pem()
}
