//! App definitions: bundles, where they came from, and fetching them.

pub mod bundle;
pub mod provenance;
pub mod version;

pub use bundle::{Bundle, BundleError, Script};
pub use provenance::{Origin, Source};
pub use version::VersionRequirement;
