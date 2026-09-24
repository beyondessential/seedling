use rhai::{EvalAltResult, TypeBuilder};

use super::{
    super::file::{Directory, File},
    App,
};
use crate::runtime::definition::bundle::normalise_path;

fn bundle_path(path: &str) -> Result<String, Box<EvalAltResult>> {
    normalise_path(path).map_err(|e| format!("bundle {e}").into())
}

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl app.file]
    // l[impl app.bundle.context]
    builder.with_fn(
        "file",
        |this: &mut App, path: &str| -> Result<File, Box<EvalAltResult>> {
            let normalised = bundle_path(path)?;
            let contents = this
                .bundle
                .file(&normalised)
                .ok_or_else(|| -> Box<EvalAltResult> {
                    format!("the definition bundle has no file '{path}'").into()
                })?;
            Ok(File {
                path: normalised,
                contents: contents.clone(),
            })
        },
    );

    // l[impl app.dir]
    // l[impl app.bundle.context]
    builder.with_fn(
        "dir",
        |this: &mut App, path: &str| -> Result<Directory, Box<EvalAltResult>> {
            let normalised = bundle_path(path)?;
            let prefix = if normalised.is_empty() {
                String::new()
            } else {
                format!("{normalised}/")
            };
            let files: Vec<_> = this
                .bundle
                .files()
                .iter()
                .filter_map(|(p, c)| {
                    p.strip_prefix(&prefix)
                        .map(|rel| (rel.to_owned(), c.clone()))
                })
                .collect();
            if files.is_empty() {
                return Err(format!("the definition bundle has no files beneath '{path}'").into());
            }
            Ok(Directory {
                path: normalised,
                files,
            })
        },
    );
}
