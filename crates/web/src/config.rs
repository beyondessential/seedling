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

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn write_config(contents: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        file
    }

    #[test]
    fn auth_section_without_lifetime_uses_default() {
        let file = write_config("[auth]\n");
        let config = Config::from_file(file.path()).unwrap();
        assert_eq!(config.auth.password_hash, None);
        assert_eq!(config.auth.session_lifetime_secs, 86400);
    }

    #[test]
    fn auth_section_parses_all_fields() {
        let file = write_config(
            r#"
            [auth]
            password_hash = "$argon2id$stub"
            session_lifetime_secs = 3600
            "#,
        );
        let config = Config::from_file(file.path()).unwrap();
        assert_eq!(config.auth.password_hash.as_deref(), Some("$argon2id$stub"));
        assert_eq!(config.auth.session_lifetime_secs, 3600);
    }

    #[test]
    fn invalid_toml_reports_path_in_error() {
        let file = write_config("[auth\nbroken");
        let err = Config::from_file(file.path()).unwrap_err();
        assert!(
            err.0.contains("parse") && err.0.contains(&file.path().display().to_string()),
            "got: {err}"
        );
    }

    #[test]
    fn missing_file_reports_read_error() {
        let err =
            Config::from_file(std::path::Path::new("/nonexistent/seedling-web.toml")).unwrap_err();
        assert!(err.0.starts_with("read "), "got: {err}");
    }
}
