use serde::Serialize;

// i[wire.error-codes]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotFound,
    NotInstalled,
    AlreadyInstalled,
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
