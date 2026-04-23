use rhai::{Engine, Scope};

pub mod defs;
pub mod oi;
pub mod runtime;
pub mod system;

#[cfg(test)]
mod tests;

/// Configurable limits for the Rhai script engine.
// r[impl engine.limits]
#[derive(Debug, Clone)]
pub struct ScriptLimits {
    // r[impl engine.limits.operations]
    pub max_operations: u64,
    // r[impl engine.limits.call-depth]
    pub max_call_levels: usize,
    // r[impl engine.limits.expr-depth]
    pub max_expr_depth: usize,
    // r[impl engine.limits.string-size]
    pub max_string_size: usize,
    // r[impl engine.limits.array-size]
    pub max_array_size: usize,
    // r[impl engine.limits.map-size]
    pub max_map_size: usize,
}

impl Default for ScriptLimits {
    fn default() -> Self {
        Self {
            max_operations: 100_000,
            max_call_levels: 64,
            max_expr_depth: 64,
            max_string_size: 1_048_576,
            max_array_size: 10_000,
            max_map_size: 10_000,
        }
    }
}

pub fn setup_language(limits: &ScriptLimits) -> (Engine, Scope<'static>, defs::app::App) {
    let mut engine = Engine::new();

    engine.set_max_operations(limits.max_operations);
    engine.set_max_call_levels(limits.max_call_levels);
    engine.set_max_expr_depths(limits.max_expr_depth, limits.max_expr_depth);
    engine.set_max_string_size(limits.max_string_size);
    engine.set_max_array_size(limits.max_array_size);
    engine.set_max_map_size(limits.max_map_size);

    defs::register(&mut engine);
    let (scope, app) = defs::scope();
    (engine, scope, app)
}

#[cfg(test)]
mod engine_limits_tests {
    use super::*;

    fn eval<'a>(limits: ScriptLimits, src: &'a str) -> Result<(), Box<rhai::EvalAltResult>> {
        let (engine, mut scope, _app) = setup_language(&limits);
        let ast = engine.compile(src)?;
        engine.run_ast_with_scope(&mut scope, &ast)
    }

    // r[verify engine.limits]
    // r[verify engine.limits.operations]
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

    // r[verify engine.limits.call-depth]
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

    // r[verify engine.limits.expr-depth]
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
                rhai::EvalAltResult::ErrorParsing(
                    rhai::ParseErrorType::ExprTooDeep,
                    _,
                ),
            ),
            "expected ExprTooDeep, got {err:?}",
        );
    }

    // r[verify engine.limits.string-size]
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

    // r[verify engine.limits.array-size]
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

    // r[verify engine.limits.map-size]
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
                rhai::EvalAltResult::ErrorParsing(
                    rhai::ParseErrorType::LiteralTooLarge(..),
                    _,
                ),
            ),
            "expected ParseError LiteralTooLarge, got {err:?}",
        );
    }

    // r[verify engine.limits]
    #[test]
    fn default_limits_allow_normal_scripts() {
        eval(
            ScriptLimits::default(),
            r#"app.deployment("web").image("docker.io/library/nginx:latest");"#,
        )
        .expect("normal script should run within default limits");
    }
}
