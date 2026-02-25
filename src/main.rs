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

    engine.build_type::<defs::App>();
    engine.build_type::<defs::Service>();
    engine.register_fn("__app", || defs::App::default());
    dbg!(engine.gen_fn_signatures(false));

    let mut scope = Scope::new();
    let _ = engine.run_with_scope(&mut scope, "let app = __app();");

    engine.eval_file_with_scope(&mut scope, filepath)?;

    Ok(())
}
