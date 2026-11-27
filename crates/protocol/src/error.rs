use serde::Serialize;

// spec: WIRE#error-codes
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
    // spec: PRM#validation
    ValidationFailed,
    // spec: WIRE#error-codes
    EvaluationRunaway,
    // spec: DEFN#bundle-rules
    BundleInvalid,
    // spec: DEFN#seedling-version-support
    UnsupportedSeedling,
    // spec: DEFN#fetching
    FetchFailed,
    // spec: IMGM#removing-an-image
    ImageInUse,
    // spec: DEFN#registry-access
    RegistryNotAllowed,
    ServerBusy,
    Internal,
    // spec: RLY#disabled-access
    // spec: RLY#unavailability
    CanopyDisabled,
}

impl ErrorCode {
    /// The name the code goes by on the wire.
    ///
    /// Matched exhaustively, so a code added to the enum does not compile
    /// until it is named here, and the serialisation test holds this to what
    /// serde writes.
    // spec: WIRE#error-codes
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::NotInstalled => "not_installed",
            Self::AlreadyInstalled => "already_installed",
            Self::OperationInProgress => "operation_in_progress",
            Self::AlreadyQueued => "already_queued",
            Self::RequirementsInvalid => "requirements_invalid",
            Self::ScriptError => "script_error",
            Self::ValidationFailed => "validation_failed",
            Self::EvaluationRunaway => "evaluation_runaway",
            Self::BundleInvalid => "bundle_invalid",
            Self::UnsupportedSeedling => "unsupported_seedling",
            Self::FetchFailed => "fetch_failed",
            Self::ImageInUse => "image_in_use",
            Self::RegistryNotAllowed => "registry_not_allowed",
            Self::ServerBusy => "server_busy",
            Self::Internal => "internal",
            Self::CanopyDisabled => "canopy_disabled",
        }
    }
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

    /// A failure inside the server, such as a failed database read: never
    /// an answer about what was asked for.
    // spec: WIRE#error-codes
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
}

pub type HandlerResult = Result<serde_json::Value, OiError>;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // spec: WIRE#error-codes
    #[test]
    fn error_codes_serialise_as_snake_case_strings() {
        let cases = [
            (ErrorCode::NotFound, "not_found"),
            (ErrorCode::NotInstalled, "not_installed"),
            (ErrorCode::AlreadyInstalled, "already_installed"),
            (ErrorCode::OperationInProgress, "operation_in_progress"),
            (ErrorCode::AlreadyQueued, "already_queued"),
            (ErrorCode::RequirementsInvalid, "requirements_invalid"),
            (ErrorCode::ScriptError, "script_error"),
            (ErrorCode::ValidationFailed, "validation_failed"),
            (ErrorCode::EvaluationRunaway, "evaluation_runaway"),
            (ErrorCode::BundleInvalid, "bundle_invalid"),
            (ErrorCode::UnsupportedSeedling, "unsupported_seedling"),
            (ErrorCode::FetchFailed, "fetch_failed"),
            (ErrorCode::ImageInUse, "image_in_use"),
            (ErrorCode::RegistryNotAllowed, "registry_not_allowed"),
            (ErrorCode::ServerBusy, "server_busy"),
            (ErrorCode::Internal, "internal"),
            (ErrorCode::CanopyDisabled, "canopy_disabled"),
        ];
        for (code, expected) in cases {
            assert_eq!(serde_json::to_value(code).unwrap(), json!(expected));
            assert_eq!(code.wire_name(), expected);
        }
    }
}
