use std::path::PathBuf;

use rhai::{Engine, EvalAltResult};

mod defs;

fn main() -> Result<(), Box<EvalAltResult>> {
    let filepath = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("Usage: beset <RHAI FILE>"),
    );

    let mut engine = Engine::new();
    defs::register(&mut engine);

    let (mut scope, app) = defs::scope();
    let res = engine.run_file_with_scope(&mut scope, filepath);
    dbg!(app);
    if let Err(err) = res {
        eprintln!("{err}");
    }

    Ok(())
}
