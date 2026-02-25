use std::path::PathBuf;

use rhai::{Engine, EvalAltResult, Scope};

mod defs;

fn main() -> Result<(), Box<EvalAltResult>> {
    let filepath = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("Usage: beset <RHAI FILE>"),
    );

    let mut engine = Engine::new();
    defs::register(&mut engine);

    let mut scope = Scope::new();
    let _ = engine.run_with_scope(&mut scope, "let app = __app();");

    let res = engine.eval_file_with_scope::<()>(&mut scope, filepath);
    let app: defs::app::App = engine.eval_with_scope(&mut scope, "app")?;
    dbg!(app);
    if let Err(err) = res {
        eprintln!("{err}");
    }

    Ok(())
}
