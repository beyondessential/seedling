//! Comparison operators shared by the BSL value types that compare only with
//! a value of their own type.

use rhai::{CustomType, Dynamic, EvalAltResult, NativeCallContext, TypeBuilder};

const OPERATORS: [&str; 6] = ["==", "!=", "<", "<=", ">", ">="];

/// Register every comparison operator between a `T`, which a script knows as
/// `name`, and a value of any other type, on either side, each raising a
/// script error that ends with `rule`.
///
/// Rhai picks an operator registered for both operand types over one taking a
/// `Dynamic`, so these apply only where the other operand is not a `T`.
pub(crate) fn refuse_mixed_comparisons<T>(
    builder: &mut TypeBuilder<T>,
    name: &'static str,
    rule: &'static str,
) where
    T: CustomType + Clone + Send + Sync + 'static,
{
    for op in OPERATORS {
        builder
            .with_fn(
                op,
                move |ctx: NativeCallContext,
                      _: &mut T,
                      b: Dynamic|
                      -> Result<bool, Box<EvalAltResult>> {
                    Err(mixed(op, name, type_name(&ctx, &b), rule))
                },
            )
            .with_fn(
                op,
                move |ctx: NativeCallContext,
                      a: Dynamic,
                      _: T|
                      -> Result<bool, Box<EvalAltResult>> {
                    Err(mixed(op, type_name(&ctx, &a), name, rule))
                },
            );
    }
}

fn mixed(op: &str, left: &str, right: &str, rule: &str) -> Box<EvalAltResult> {
    format!("cannot compare {left} {op} {right}: {rule}").into()
}

/// The name a script knows `value`'s type by: the name its type was
/// registered with, or Rhai's own name for a built-in type.
pub(crate) fn type_name<'a>(ctx: &'a NativeCallContext, value: &'a Dynamic) -> &'a str {
    ctx.engine().map_type_name(value.type_name())
}
