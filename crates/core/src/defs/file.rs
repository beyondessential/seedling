use bytes::Bytes;
use rhai::{CustomType, EvalAltResult, TypeBuilder};

/// A file read from the app's definition bundle.
// l[impl file.type]
#[derive(Debug, Clone)]
pub struct File {
    pub path: String,
    pub contents: Bytes,
}

impl CustomType for File {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("File")
            // l[impl file.text]
            .with_fn(
                "text",
                |this: &mut Self| -> Result<String, Box<EvalAltResult>> {
                    String::from_utf8(this.contents.to_vec()).map_err(|_| {
                        format!("bundle file '{}' is not valid UTF-8", this.path).into()
                    })
                },
            );
    }
}

/// Every file beneath a directory of the definition bundle, each at its path
/// relative to that directory.
// l[impl app.dir]
#[derive(Debug, Clone)]
pub struct Directory {
    pub path: String,
    pub files: Vec<(String, Bytes)>,
}

impl CustomType for Directory {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name("Directory");
    }
}
