use rhai::{Engine, Scope};

pub mod defs;
pub mod oi;
pub mod reserved;
pub mod runtime;
pub mod sysconst;
pub mod system;

#[cfg(test)]
#[expect(
    clippy::disallowed_methods,
    reason = "tests evaluate snippets of BSL directly, not definitions"
)]
mod tests;

/// Configurable limits for the Rhai script engine, read from the options the
/// runtime and the CLI's definition check share.
// spec: LIM#limits
#[derive(Debug, Clone, clap::Args)]
pub struct ScriptLimits {
    /// Maximum operations per script evaluation (0 = unlimited)
    // spec: LIM#limits
    #[arg(long = "script-max-operations", default_value_t = ScriptLimits::DEFAULT.max_operations)]
    pub max_operations: u64,
    /// Maximum function call nesting depth (at least 1)
    // spec: LIM#limits
    #[arg(long = "script-max-call-depth", default_value_t = ScriptLimits::DEFAULT.max_call_levels, value_parser = parse_call_depth)]
    pub max_call_levels: usize,
    /// Maximum expression nesting depth (0 = unlimited)
    // spec: LIM#limits
    #[arg(long = "script-max-expr-depth", default_value_t = ScriptLimits::DEFAULT.max_expr_depth)]
    pub max_expr_depth: usize,
    /// Maximum string size in bytes (0 = unlimited)
    // spec: LIM#limits
    #[arg(long = "script-max-string-size", default_value_t = ScriptLimits::DEFAULT.max_string_size)]
    pub max_string_size: usize,
    /// Maximum array size in elements (0 = unlimited)
    // spec: LIM#limits
    #[arg(long = "script-max-array-size", default_value_t = ScriptLimits::DEFAULT.max_array_size)]
    pub max_array_size: usize,
    /// Maximum object map size in entries (0 = unlimited)
    // spec: LIM#limits
    #[arg(long = "script-max-map-size", default_value_t = ScriptLimits::DEFAULT.max_map_size)]
    pub max_map_size: usize,
    /// Maximum calls to a derivation helper per evaluation (0 = unlimited)
    // spec: LIM#limits
    #[arg(long = "script-max-derived-values", default_value_t = ScriptLimits::DEFAULT.max_derived_values)]
    pub max_derived_values: usize,
    /// Maximum calls to a password hash helper per evaluation (0 = unlimited)
    // spec: LIM#limits
    #[arg(long = "script-max-password-hashes", default_value_t = ScriptLimits::DEFAULT.max_password_hashes)]
    pub max_password_hashes: usize,
    /// Maximum bytes of patterns compiled by `std::regex` per evaluation (0 = unlimited)
    // spec: LIM#limits
    #[arg(long = "script-max-regex-compiled-size", default_value_t = ScriptLimits::DEFAULT.max_regex_compiled_size)]
    pub max_regex_compiled_size: usize,
}

impl Default for ScriptLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

// spec: LIM#limits
fn parse_call_depth(raw: &str) -> Result<usize, String> {
    let depth: usize = raw.parse().map_err(|e| format!("{e}"))?;
    if depth == 0 {
        return Err("a call depth of 0 would allow no function calls".to_owned());
    }
    Ok(depth)
}

impl ScriptLimits {
    /// The limits an evaluation is held to where no option sets them.
    pub const DEFAULT: Self = Self {
        max_operations: 100_000,
        max_call_levels: 64,
        max_expr_depth: 64,
        max_string_size: 1_048_576,
        max_array_size: 10_000,
        max_map_size: 10_000,
        max_derived_values: 1_000,
        max_password_hashes: 16,
        max_regex_compiled_size: 33_554_432,
    };

    /// The limits the `std` helpers and the kind parsers hold values to.
    pub fn helpers(&self) -> defs::stdlib::HelperLimits {
        defs::stdlib::HelperLimits {
            derived_values: self.max_derived_values,
            password_hashes: self.max_password_hashes,
            regex_compiled_size: self.max_regex_compiled_size,
        }
    }
}

/// An engine for evaluating a script, and the scope and app it evaluates
/// into; fails where the scope cannot be built, as [`defs::scope`] does.
pub fn setup_language(
    limits: &ScriptLimits,
    bundle: std::sync::Arc<seedling_bundle::Bundle>,
) -> Result<(Engine, Scope<'static>, defs::app::App), String> {
    let mut engine = Engine::new();

    engine.set_max_operations(limits.max_operations);
    engine.set_max_call_levels(limits.max_call_levels);
    engine.set_max_expr_depths(limits.max_expr_depth, limits.max_expr_depth);
    engine.set_max_string_size(limits.max_string_size);
    engine.set_max_array_size(limits.max_array_size);
    engine.set_max_map_size(limits.max_map_size);
    engine.set_default_tag(rhai::Dynamic::from(limits.helpers()));
    // spec: REX#compiling-a-pattern
    // Optimisation would carry a `const` into a call's argument, which the
    // literal-pattern check must not take for a literal, so `defs::compile`
    // optimises once it has checked.
    engine.set_optimization_level(rhai::OptimizationLevel::None);

    defs::register(&mut engine);
    let (scope, app) = defs::scope(bundle)?;
    Ok((engine, scope, app))
}

#[cfg(test)]
mod engine_limits_tests {
    use super::*;

    fn eval(limits: ScriptLimits, src: &str) -> Result<(), Box<rhai::EvalAltResult>> {
        let (engine, mut scope, _app) =
            setup_language(&limits, Default::default()).expect("the scope builds");
        let ast = crate::defs::compile(&engine, src)?;
        engine.run_ast_with_scope(&mut scope, &ast)
    }

    // spec: LIM#limits
    #[test]
    fn operations_limit_aborts_long_loops() {
        let limits = ScriptLimits {
            max_operations: 500,
            ..ScriptLimits::default()
        };
        let err = eval(
            limits,
            r#"
            let i = 0;
            while i < 10000 { i += 1; }
            "#,
        )
        .expect_err("should hit operations limit");
        assert!(
            matches!(*err, rhai::EvalAltResult::ErrorTooManyOperations(_)),
            "expected ErrorTooManyOperations, got {err:?}",
        );
    }

    // spec: LIM#limits
    #[test]
    fn call_depth_limit_rejects_deep_recursion() {
        let limits = ScriptLimits {
            max_call_levels: 8,
            ..ScriptLimits::default()
        };
        let err = eval(
            limits,
            r#"
            fn rec(n) { if n <= 0 { 0 } else { rec(n - 1) + 1 } }
            rec(20)
            "#,
        )
        .expect_err("should hit call-depth limit");
        assert!(
            matches!(*err, rhai::EvalAltResult::ErrorStackOverflow(..)),
            "expected ErrorStackOverflow, got {err:?}",
        );
    }

    // spec: LIM#limits
    #[test]
    fn expr_depth_limit_rejects_deeply_nested_expressions() {
        let limits = ScriptLimits {
            max_expr_depth: 4,
            ..ScriptLimits::default()
        };
        // Ten nested parentheses comfortably exceeds depth 4.
        let src = format!("let x = {}1{};", "(".repeat(10), ")".repeat(10));
        let err = eval(limits, &src).expect_err("should hit expr-depth limit");
        assert!(
            matches!(
                *err,
                rhai::EvalAltResult::ErrorParsing(rhai::ParseErrorType::ExprTooDeep, _,),
            ),
            "expected ExprTooDeep, got {err:?}",
        );
    }

    // spec: LIM#limits
    #[test]
    fn string_size_limit_rejects_oversized_strings() {
        let limits = ScriptLimits {
            max_string_size: 16,
            ..ScriptLimits::default()
        };
        let err = eval(
            limits,
            r#"
            let s = "abcdefgh";
            s + s + s;
            "#,
        )
        .expect_err("should hit string-size limit");
        assert!(
            matches!(*err, rhai::EvalAltResult::ErrorDataTooLarge(..)),
            "expected ErrorDataTooLarge, got {err:?}",
        );
    }

    // spec: LIM#limits
    #[test]
    fn array_size_limit_rejects_growth_past_cap() {
        let limits = ScriptLimits {
            max_array_size: 3,
            ..ScriptLimits::default()
        };
        let err = eval(
            limits,
            r#"
            let a = [];
            a.push(1); a.push(2); a.push(3); a.push(4);
            "#,
        )
        .expect_err("should hit array-size limit");
        assert!(
            matches!(*err, rhai::EvalAltResult::ErrorDataTooLarge(..)),
            "expected ErrorDataTooLarge, got {err:?}",
        );
    }

    // spec: LIM#limits
    #[test]
    fn map_size_limit_rejects_oversized_literal() {
        let limits = ScriptLimits {
            max_map_size: 2,
            ..ScriptLimits::default()
        };
        let err = eval(limits, r#"let m = #{ a: 1, b: 2, c: 3 };"#)
            .expect_err("should hit map-size limit");
        assert!(
            matches!(
                *err,
                rhai::EvalAltResult::ErrorParsing(rhai::ParseErrorType::LiteralTooLarge(..), _,),
            ),
            "expected ParseError LiteralTooLarge, got {err:?}",
        );
    }

    // spec: LIM#limits
    #[test]
    fn default_limits_allow_normal_scripts() {
        eval(
            ScriptLimits::default(),
            r#"app.deployment("web").image("docker.io/library/nginx:latest");"#,
        )
        .expect("normal script should run within default limits");
    }

    // spec: LIM#limits
    #[test]
    fn a_script_cannot_pause_evaluation() {
        for call in ["sleep(60)", "sleep(60.0)"] {
            let started = std::time::Instant::now();
            let err = eval(ScriptLimits::default(), call).expect_err("sleep should throw");
            assert!(
                matches!(*err, rhai::EvalAltResult::ErrorRuntime(..)),
                "expected {call} to throw, got {err:?}",
            );
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "{call} paused evaluation",
            );
        }
    }
}
