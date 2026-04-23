use rhai::{EvalAltResult, Map};

/// Shared options carried by `l[impl volume.exported]` and
/// `l[impl service.exported]`. New fields should be optional so old
/// callers keep compiling.
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub description: Option<String>,
}

impl ExportOptions {
    /// Parse the `#{ description?: string }` map accepted by both the
    /// `volume.exported(options)` and `service.exported(options)` BSL
    /// builder methods.
    pub fn from_rhai_map(options: Map) -> Result<Self, Box<EvalAltResult>> {
        let description = if let Some(desc) = options.get("description") {
            Some(
                desc.clone()
                    .into_string()
                    .map_err(|e| -> Box<EvalAltResult> {
                        format!("export description must be a string: {e}").into()
                    })?,
            )
        } else {
            None
        };
        Ok(Self { description })
    }
}
