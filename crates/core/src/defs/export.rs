use rhai::{EvalAltResult, Map};

/// Shared options carried by an exported volume and an exported service.
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub description: Option<String>,
}

impl ExportOptions {
    /// Parse the `#{ description?: string }` map accepted by both the
    /// `volume.exported(options)` and `service.exported(options)` BSL
    /// builder methods.
    // spec: VOL#export
    // spec: SVC#export
    pub fn from_rhai_map(options: Map) -> Result<Self, Box<EvalAltResult>> {
        super::take::reject_unknown_keys("exported options", &options, &["description"])?;
        let description = options
            .get("description")
            .cloned()
            .map(|desc| super::take::take_string("export description", desc))
            .transpose()?;
        Ok(Self { description })
    }
}
