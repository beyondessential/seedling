//! The values the `ip-address`, `ip-subnet` and `ip-range` kinds' parsers
//! return.

use std::{
    cmp::Ordering,
    fmt,
    net::{IpAddr, Ipv4Addr},
};

use ipnet::IpNet;
use rhai::{CustomType, Dynamic, EvalAltResult, INT, NativeCallContext, TypeBuilder};

use super::{
    compare::{refuse_mixed_comparisons, type_name},
    param_def::is_decimal_number,
    take::take_int_in_range,
};

const MIXED_RULE: &str = "an IP value compares only with a value of its own type";

/// Whether an ordering satisfies a comparison operator.
type OrderingTest = fn(Ordering) -> bool;

/// Whether an address, subnet or range is of IPv4 or of IPv6.
// spec: IP#family
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    V4,
    V6,
}

impl Family {
    fn of(addr: IpAddr) -> Self {
        match addr {
            IpAddr::V4(_) => Self::V4,
            IpAddr::V6(_) => Self::V6,
        }
    }

    /// `4` or `6`.
    pub fn number(self) -> INT {
        match self {
            Self::V4 => 4,
            Self::V6 => 6,
        }
    }

    /// The greatest prefix length of an address of the family.
    fn width(self) -> u8 {
        match self {
            Self::V4 => 32,
            Self::V6 => 128,
        }
    }
}

impl fmt::Display for Family {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::V4 => "IPv4",
            Self::V6 => "IPv6",
        })
    }
}

/// Read an IP kind's `family` argument.
// spec: IP#family-argument
pub fn take_family(what: &str, value: Dynamic) -> Result<Family, Box<EvalAltResult>> {
    let type_name = value.type_name();
    match value.as_int() {
        Ok(4) => Ok(Family::V4),
        Ok(6) => Ok(Family::V6),
        Ok(n) => Err(format!("`{what}` must be 4 or 6, got {n}").into()),
        Err(_) => Err(format!("`{what}` must be the integer 4 or 6, got {type_name}").into()),
    }
}

/// Check that a parsed value is of the family a kind's `family` asks for.
// spec: IP#family-argument
pub fn check_family(expected: Option<Family>, actual: Family, what: &str) -> Result<(), String> {
    match expected {
        Some(expected) if expected != actual => {
            Err(format!("must be an {expected} {what}, not an {actual} one"))
        }
        _ => Ok(()),
    }
}

/// An IPv4 or IPv6 address.
// spec: IP#address-values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpAddress(IpAddr);

impl IpAddress {
    const NAME: &str = "IpAddress";

    // spec: IP#addresses
    pub fn parse(text: &str) -> Result<Self, String> {
        let addr = parse_any_address(text)?;
        if let Some(v4) = mapped(addr) {
            return Err(format!(
                "{text:?} is an IPv4-mapped IPv6 address, which is written as the IPv4 address {v4}"
            ));
        }
        Ok(Self(addr))
    }

    pub fn family(&self) -> Family {
        Family::of(self.0)
    }

    /// The subnet of `prefix_len` that holds the address, with its host bits
    /// cleared.
    // spec: IP#address-values
    fn to_subnet(self, prefix_len: u8) -> IpSubnet {
        let net = IpNet::new(self.0, prefix_len).expect("the prefix length is within the family's");
        IpSubnet(net.trunc())
    }

    // spec: IP#address-values
    fn to_range(self) -> IpRange {
        IpRange {
            start: self.0,
            end: self.0,
        }
    }

    // spec: IP#address-values
    fn order_with(self, other: Self) -> Result<Ordering, Box<EvalAltResult>> {
        let (family, other_family) = (self.family(), other.family());
        if family != other_family {
            return Err(
                format!("cannot order an {family} address against an {other_family} one").into(),
            );
        }
        Ok(self.0.cmp(&other.0))
    }
}

// spec: IP#text-of-a-value
impl fmt::Display for IpAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl CustomType for IpAddress {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name(Self::NAME);

        // spec: IP#address-values
        builder.with_fn("family", |this: &mut Self| this.family().number());
        // spec: IP#address-values
        builder.with_fn("to_subnet", |this: &mut Self| {
            this.to_subnet(this.family().width())
        });
        // spec: IP#address-values
        builder.with_fn(
            "to_subnet",
            |this: &mut Self, prefix_len: INT| -> Result<IpSubnet, Box<EvalAltResult>> {
                let width = INT::from(this.family().width());
                let prefix_len = take_int_in_range("prefix_len", prefix_len, 0..=width)?;
                Ok(this.to_subnet(prefix_len))
            },
        );
        // spec: IP#address-values
        builder.with_fn("to_range", |this: &mut Self| this.to_range());

        // spec: IP#address-values
        builder
            .with_fn("==", |a: &mut Self, b: Self| *a == b)
            .with_fn("!=", |a: &mut Self, b: Self| *a != b);
        let orderings: [(&str, OrderingTest); 4] = [
            ("<", Ordering::is_lt),
            ("<=", Ordering::is_le),
            (">", Ordering::is_gt),
            (">=", Ordering::is_ge),
        ];
        for (op, holds) in orderings {
            // spec: IP#address-values
            builder.with_fn(
                op,
                move |a: &mut Self, b: Self| -> Result<bool, Box<EvalAltResult>> {
                    a.order_with(b).map(holds)
                },
            );
        }

        register_shared(&mut builder, Self::NAME);
    }
}

/// An IPv4 or IPv6 subnet, whose host bits are all zero.
// spec: IP#subnet-values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpSubnet(IpNet);

impl IpSubnet {
    const NAME: &str = "IpSubnet";

    // spec: IP#subnets
    pub fn parse(text: &str) -> Result<Self, String> {
        let (addr_text, prefix_text) = match text.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (text, None),
        };
        let addr = IpAddress::parse(addr_text)?;
        let width = addr.family().width();
        let prefix_len = match prefix_text {
            Some(prefix) => parse_prefix_len(prefix, width).ok_or_else(|| {
                format!("the prefix length of {text:?} must be a decimal number from 0 to {width}")
            })?,
            None => width,
        };
        let net = IpNet::new(addr.0, prefix_len).expect("the prefix length is within the family's");
        if net.network() != addr.0 {
            return Err(format!(
                "{text:?} has host bits set; the subnet it lies in is {}",
                Self(net.trunc())
            ));
        }
        Ok(Self(net))
    }

    pub fn family(&self) -> Family {
        Family::of(self.0.network())
    }

    // spec: IP#subnet-values
    fn to_range(self) -> IpRange {
        IpRange {
            start: self.0.network(),
            end: self.0.broadcast(),
        }
    }

    fn interval(self) -> Interval {
        Interval::new(self.0.network(), self.0.broadcast())
    }
}

// spec: IP#subnets
// spec: IP#decimal-number
fn parse_prefix_len(text: &str, width: u8) -> Option<u8> {
    if !is_decimal_number(text) {
        return None;
    }
    text.parse().ok().filter(|&n| n <= width)
}

// spec: IP#text-of-a-value
impl fmt::Display for IpSubnet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.0.network(), self.0.prefix_len())
    }
}

impl CustomType for IpSubnet {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name(Self::NAME);

        // spec: IP#subnet-values
        builder.with_fn("family", |this: &mut Self| this.family().number());
        // spec: IP#subnet-values
        builder.with_fn("address", |this: &mut Self| IpAddress(this.0.network()));
        // spec: IP#subnet-values
        builder.with_fn("prefix_len", |this: &mut Self| {
            INT::from(this.0.prefix_len())
        });
        // spec: IP#subnet-values
        builder.with_fn("to_range", |this: &mut Self| this.to_range());
        register_containment(&mut builder, |this| this.interval());

        // spec: IP#subnet-values
        register_unordered(&mut builder, "subnets");
        register_shared(&mut builder, Self::NAME);
    }
}

/// A range of IPv4 or IPv6 addresses, from its first address to its last,
/// both of one family.
// spec: IP#range-values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpRange {
    start: IpAddr,
    end: IpAddr,
}

impl IpRange {
    const NAME: &str = "IpRange";

    // spec: IP#ranges
    pub fn parse(text: &str) -> Result<Self, String> {
        if let Some((start, end)) = text.split_once('-') {
            let (start, end) = (parse_any_address(start)?, parse_any_address(end)?);
            if Family::of(start) != Family::of(end) {
                return Err(format!("{text:?} joins an IPv4 and an IPv6 address"));
            }
            // spec: IP#address-order
            if start > end {
                return Err(format!("the first address of {text:?} follows its last"));
            }
            if mapped(start).is_some() && mapped(end).is_some() {
                return Err(format!(
                    "{text:?} lies wholly among IPv4-mapped IPv6 addresses, which are written as IPv4 addresses"
                ));
            }
            return Ok(Self { start, end });
        }
        if text.contains('/') {
            IpSubnet::parse(text).map(IpSubnet::to_range)
        } else {
            IpAddress::parse(text).map(IpAddress::to_range)
        }
    }

    pub fn family(&self) -> Family {
        Family::of(self.start)
    }

    fn interval(self) -> Interval {
        Interval::new(self.start, self.end)
    }
}

// spec: IP#text-of-a-value
impl fmt::Display for IpRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.start, self.end)
    }
}

impl CustomType for IpRange {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name(Self::NAME);

        // spec: IP#range-values
        builder.with_fn("family", |this: &mut Self| this.family().number());
        // spec: IP#range-values
        builder.with_fn("start", |this: &mut Self| IpAddress(this.start));
        // spec: IP#range-values
        builder.with_fn("end", |this: &mut Self| IpAddress(this.end));
        register_containment(&mut builder, |this| this.interval());

        // spec: IP#range-values
        register_unordered(&mut builder, "ranges");
        register_shared(&mut builder, Self::NAME);
    }
}

/// Every address an IP value holds, as an inclusive interval in address
/// order within its family.
// spec: IP#address-order
#[derive(Debug, Clone, Copy)]
struct Interval {
    family: Family,
    first: u128,
    last: u128,
}

impl Interval {
    fn new(first: IpAddr, last: IpAddr) -> Self {
        Self {
            family: Family::of(first),
            first: address_order(first),
            last: address_order(last),
        }
    }

    // spec: IP#containment-and-overlap
    fn contains(self, other: Self) -> bool {
        self.family == other.family && self.first <= other.first && other.last <= self.last
    }

    // spec: IP#containment-and-overlap
    fn overlaps(self, other: Self) -> bool {
        self.family == other.family && other.first <= self.last && self.first <= other.last
    }

    /// The interval of a script's `IpAddress`, `IpSubnet` or `IpRange`.
    // spec: IP#containment-and-overlap
    fn take(
        ctx: &NativeCallContext,
        what: &str,
        value: Dynamic,
    ) -> Result<Self, Box<EvalAltResult>> {
        // take: dispatch
        if let Some(addr) = value.clone().try_cast::<IpAddress>() {
            return Ok(Self::new(addr.0, addr.0));
        }
        // take: dispatch
        if let Some(subnet) = value.clone().try_cast::<IpSubnet>() {
            return Ok(subnet.interval());
        }
        // take: dispatch
        if let Some(range) = value.clone().try_cast::<IpRange>() {
            return Ok(range.interval());
        }
        Err(format!(
            "`{what}` must be an IpAddress, IpSubnet or IpRange, got {}",
            type_name(ctx, &value)
        )
        .into())
    }
}

/// An address as an unsigned integer of its family's width.
// spec: IP#address-order
fn address_order(addr: IpAddr) -> u128 {
    match addr {
        IpAddr::V4(addr) => u32::from(addr).into(),
        IpAddr::V6(addr) => addr.into(),
    }
}

/// An address in the IPv6 text form or as four dotted decimal numbers,
/// IPv4-mapped or not.
// spec: IP#addresses
fn parse_any_address(text: &str) -> Result<IpAddr, String> {
    text.parse()
        .map_err(|_| format!("{text:?} is not an IP address"))
}

/// The IPv4 address an IPv4-mapped IPv6 address carries.
fn mapped(addr: IpAddr) -> Option<Ipv4Addr> {
    match addr {
        IpAddr::V4(_) => None,
        IpAddr::V6(addr) => addr.to_ipv4_mapped(),
    }
}

/// Register `contains` and `overlaps` on a subnet or range.
fn register_containment<T>(builder: &mut TypeBuilder<T>, interval: fn(T) -> Interval)
where
    T: CustomType + Copy + Send + Sync + 'static,
{
    // spec: IP#containment-and-overlap
    builder
        .with_fn(
            "contains",
            move |ctx: NativeCallContext,
                  this: &mut T,
                  other: Dynamic|
                  -> Result<bool, Box<EvalAltResult>> {
                Ok(interval(*this).contains(Interval::take(&ctx, "other", other)?))
            },
        )
        .with_fn(
            "overlaps",
            move |ctx: NativeCallContext,
                  this: &mut T,
                  other: Dynamic|
                  -> Result<bool, Box<EvalAltResult>> {
                Ok(interval(*this).overlaps(Interval::take(&ctx, "other", other)?))
            },
        );
}

/// Register equality on a subnet or range, and the ordering operators, which
/// raise a script error.
fn register_unordered<T>(builder: &mut TypeBuilder<T>, plural: &'static str)
where
    T: CustomType + PartialEq + Clone + Send + Sync + 'static,
{
    builder
        .with_fn("==", |a: &mut T, b: T| *a == b)
        .with_fn("!=", |a: &mut T, b: T| *a != b);
    for op in ["<", "<=", ">", ">="] {
        builder.with_fn(
            op,
            move |_: &mut T, _: T| -> Result<bool, Box<EvalAltResult>> {
                Err(format!("{plural} have no order, so {op} cannot compare them").into())
            },
        );
    }
}

/// Register the refusal of comparisons with any other type, and how the
/// value prints.
fn register_shared<T>(builder: &mut TypeBuilder<T>, name: &'static str)
where
    T: CustomType + Clone + fmt::Display + Send + Sync + 'static,
{
    // spec: IP#mixing-types
    refuse_mixed_comparisons(builder, name, MIXED_RULE);

    // spec: IP#text-of-a-value
    builder
        .on_print(|this: &mut T| this.to_string())
        .on_debug(|this: &mut T| this.to_string());
}
