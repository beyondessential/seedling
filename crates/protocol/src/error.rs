use serde::Serialize;

// i[wire.error-codes]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotFound,
    NotInstalled,
    AlreadyInstalled,
    // i[wire.error-codes] InstallInProgress distinguishes "install is running
    // right now" from "install already succeeded (or uninstall is in flight)".
    InstallInProgress,
    OperationInProgress,
    AlreadyQueued,
    RequirementsInvalid,
    ScriptError,
    Deregistering,
    ServerBusy,
    Internal,
    // i[impl backup.app.deregister]
    BackupAppInUse,
}

#[derive(Debug)]
pub struct OiError {
    pub code: ErrorCode,
    pub message: String,
}

impl OiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    pub fn script_error(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ScriptError, message)
    }
}

pub type HandlerResult = Result<serde_json::Value, OiError>;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // i[verify wire.error-codes]
    #[test]
    fn error_codes_serialise_as_snake_case_strings() {
        let cases = [
            (ErrorCode::NotFound, "not_found"),
            (ErrorCode::NotInstalled, "not_installed"),
            (ErrorCode::AlreadyInstalled, "already_installed"),
            (ErrorCode::InstallInProgress, "install_in_progress"),
            (ErrorCode::OperationInProgress, "operation_in_progress"),
            (ErrorCode::AlreadyQueued, "already_queued"),
            (ErrorCode::RequirementsInvalid, "requirements_invalid"),
            (ErrorCode::ScriptError, "script_error"),
            (ErrorCode::Deregistering, "deregistering"),
            (ErrorCode::ServerBusy, "server_busy"),
            (ErrorCode::Internal, "internal"),
            (ErrorCode::BackupAppInUse, "backup_app_in_use"),
        ];
        for (code, expected) in cases {
            assert_eq!(serde_json::to_value(code).unwrap(), json!(expected));
        }
    }
}
