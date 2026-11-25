use semver::Version;

/// The running Seedling's version, as the workspace declares it.
pub fn running() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is valid semver")
}
