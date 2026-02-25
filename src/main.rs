use std::path::PathBuf;

use rhai::{Engine, EvalAltResult, Scope};

use crate::defs::App;

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
    engine.build_type::<defs::Ingress>();
    engine.build_type::<defs::Deployment>();
    engine.register_fn("__app", || defs::App::default());
    dbg!(engine.gen_fn_signatures(false));

    let mut scope = Scope::new();
    let _ = engine.run_with_scope(&mut scope, "let app = __app();");

    let res = engine.eval_file_with_scope::<()>(&mut scope, filepath);
    let app: App = engine.eval_with_scope(&mut scope, "app")?;
    dbg!(app);
    if let Err(err) = res {
        eprintln!("{err}");
    }

    Ok(())
}
