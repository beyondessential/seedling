use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub auth: AuthConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct AuthConfig {
    pub password_hash: Option<String>,
    #[expect(dead_code, reason = "reserved for future signed-token support")]
    pub session_secret: Option<String>,
    /// Lifetime of long-lived session tokens in seconds. Default 86400 (1 day).
    #[serde(default = "default_session_lifetime")]
    pub session_lifetime_secs: u64,
}

fn default_session_lifetime() -> u64 {
    86400
}

impl Config {
    pub fn from_file(path: &std::path::Path) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| ConfigError(format!("read {}: {e}", path.display())))?;
        toml::from_str(&raw).map_err(|e| ConfigError(format!("parse {}: {e}", path.display())))
    }
}

#[derive(Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for ConfigError {}
