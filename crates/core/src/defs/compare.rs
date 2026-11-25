//! Comparison operators shared by the BSL value types that compare only with
//! a value of their own type.

use rhai::{CustomType, Dynamic, EvalAltResult, TypeBuilder};

use super::{
    ip::{IpAddress, IpRange, IpSubnet},
    version::{SemVer, Version},
};

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
                move |_: &mut T, b: Dynamic| -> Result<bool, Box<EvalAltResult>> {
                    Err(mixed(op, name, type_name(&b), rule))
                },
            )
            .with_fn(
                op,
                move |a: Dynamic, _: T| -> Result<bool, Box<EvalAltResult>> {
                    Err(mixed(op, type_name(&a), name, rule))
                },
            );
    }
}

fn mixed(op: &str, left: &str, right: &str, rule: &str) -> Box<EvalAltResult> {
    format!("cannot compare {left} {op} {right}: {rule}").into()
}

/// The name a script knows `value`'s type by.
pub(crate) fn type_name(value: &Dynamic) -> &str {
    if value.is::<Version>() {
        Version::NAME
    } else if value.is::<SemVer>() {
        SemVer::NAME
    } else if value.is::<IpAddress>() {
        IpAddress::NAME
    } else if value.is::<IpSubnet>() {
        IpSubnet::NAME
    } else if value.is::<IpRange>() {
        IpRange::NAME
    } else {
        value.type_name()
    }
}
