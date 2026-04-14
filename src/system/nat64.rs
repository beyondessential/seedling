use std::fmt;

use clap::ValueEnum;

/// Controls whether the runtime provides its own NAT64 translator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum Nat64Mode {
    /// Probe for existing NAT64 infrastructure on startup; enable if absent.
    #[default]
    Auto,
    /// Always provide NAT64.
    Enabled,
    /// Never provide NAT64.
    Disabled,
}

impl fmt::Display for Nat64Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Enabled => f.write_str("enabled"),
            Self::Disabled => f.write_str("disabled"),
        }
    }
}
