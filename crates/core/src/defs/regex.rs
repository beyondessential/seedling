//! Regular expressions: `std::regex`, the `Regex` values it returns, and the
//! budget their compiling counts toward.

use std::{
    collections::{HashMap, VecDeque},
    fmt,
    sync::{Arc, LazyLock, Mutex},
};

use regex_automata::{
    PatternID,
    meta::{self, BuildError},
    util::captures::Captures,
};
use rhai::{
    CustomType, Dynamic, EvalAltResult, FnPtr, FuncRegistration, INT, NativeCallContext,
    TypeBuilder,
};
use winnow::{
    Parser,
    combinator::{alt, delimited, preceded, repeat},
    token::{take_till, take_while},
};

use crate::{
    defs::{
        stdlib::{ARRAY, HelperLimits, STRING, helper_limits, max_string, within},
        take::{take_closure, take_string},
    },
    runtime::seeds,
};

/// The most bytes the cached patterns may hold, evicting the least recently
/// used to make room for a new one.
const CACHE_CAPACITY: usize = 256 << 20;

/// The DFA size the `regex` crate allows by default, which the engine builds
/// beside the NFA where it fits.
const DFA_SIZE_LIMIT: usize = 2 << 20;

/// The lazy DFA cache size the `regex` crate allows by default, which a
/// pattern grows into as it is searched, forward and in reverse.
const HYBRID_CACHE_CAPACITY: usize = 2 << 20;

/// What a cached pattern may come to hold beyond its compiled size once it
/// has been searched: its forward and reverse lazy DFA caches at capacity.
const SEARCH_ALLOWANCE: usize = 2 * HYBRID_CACHE_CAPACITY;

/// A compiled regular expression.
#[derive(Clone)]
pub struct Regex(Arc<Compiled>);

struct Compiled {
    pattern: String,
    regex: meta::Regex,
    /// The bytes the compiled pattern occupies: the engine's measure of it,
    /// with the pattern's text and this value.
    size: usize,
}

impl Regex {
    pub const NAME: &'static str = "Regex";

    pub fn pattern(&self) -> &str {
        &self.0.pattern
    }
}

impl PartialEq for Regex {
    fn eq(&self, other: &Self) -> bool {
        self.0.pattern == other.0.pattern
    }
}

impl Eq for Regex {}

impl fmt::Debug for Regex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple(Self::NAME).field(&self.0.pattern).finish()
    }
}

// spec: REX#compiling-a-pattern
impl fmt::Display for Regex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.pattern)
    }
}

/// What remains of the compiled size limit, `None` being no limit.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    remaining: Option<usize>,
}

impl Budget {
    /// The whole of the compiled size limit `limits` hold.
    pub fn of(limits: &HelperLimits) -> Self {
        Self::fresh(limits.regex_compiled_size)
    }

    /// The whole of `limit`, 0 being no limit.
    fn fresh(limit: usize) -> Self {
        Self {
            remaining: (limit != 0).then_some(limit),
        }
    }

    pub fn unlimited() -> Self {
        Self { remaining: None }
    }

    // spec: REX#compiled-size
    /// Use up what remains, giving how much that was, or `None` where there
    /// is no limit to use up.
    fn use_up(&mut self) -> Option<usize> {
        let left = self.remaining?;
        self.remaining = Some(0);
        Some(left)
    }
}

// spec: REX#compiled-size
/// Run `f` with what remains of the calling evaluation's compiled size limit,
/// and count what it used.
pub fn with_evaluation_budget<R>(
    ctx: &NativeCallContext,
    f: impl FnOnce(&mut Budget) -> R,
) -> Result<R, Box<EvalAltResult>> {
    let limit = helper_limits(ctx)?.regex_compiled_size;
    let mut budget = Budget {
        remaining: (limit != 0).then(|| limit.saturating_sub(seeds::regex_compiled())),
    };
    let result = f(&mut budget);
    if let Some(remaining) = budget.remaining {
        seeds::set_regex_compiled(limit - remaining);
    }
    Ok(result)
}

/// The helper limits of an engine [`crate::setup_language`] built.
pub fn engine_limits(engine: &rhai::Engine) -> Result<HelperLimits, String> {
    engine
        .default_tag()
        .read_lock::<HelperLimits>()
        .map(|limits| *limits)
        .ok_or_else(|| "this engine carries no limits for its helpers".to_owned())
}

/// Compiled patterns by their text, shared by every evaluation.
///
/// A hit counts the size the miss measured, so whether a pattern was cached
/// never shows in what an evaluation counts.
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Mutex::default);

#[derive(Default)]
struct Cache {
    patterns: HashMap<String, Arc<Compiled>>,
    /// The cached patterns, least recently used first.
    order: VecDeque<String>,
    /// What the cached patterns may hold, each its [`Cache::resident`] size.
    size: usize,
}

impl Cache {
    /// What `compiled` may hold while cached, once searched.
    fn resident(compiled: &Compiled) -> usize {
        compiled.size + SEARCH_ALLOWANCE
    }

    /// The cached `pattern`, marked as the most recently used.
    fn get(&mut self, pattern: &str) -> Option<Arc<Compiled>> {
        let compiled = self.patterns.get(pattern)?;
        if let Some(at) = self.order.iter().position(|p| p == pattern) {
            let used = self.order.remove(at).expect("the position is in range");
            self.order.push_back(used);
        }
        Some(Arc::clone(compiled))
    }

    /// Hold `compiled`, evicting the least recently used patterns until it
    /// fits; one that could never fit is not held. Another evaluation may
    /// have cached the same pattern meanwhile, which is kept.
    fn insert(&mut self, compiled: &Arc<Compiled>) {
        let resident = Self::resident(compiled);
        if resident > CACHE_CAPACITY || self.patterns.contains_key(&compiled.pattern) {
            return;
        }
        while self.size + resident > CACHE_CAPACITY {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(evicted) = self.patterns.remove(&oldest) {
                self.size -= Self::resident(&evicted);
            }
        }
        self.size += resident;
        self.order.push_back(compiled.pattern.clone());
        self.patterns
            .insert(compiled.pattern.clone(), Arc::clone(compiled));
    }
}

// spec: REX#compiling-a-pattern
// spec: REX#compiled-size
/// Compile `pattern` within `budget`, counting its size toward it.
///
/// A pattern too large for what remains uses the rest of the budget up.
pub fn compile(pattern: &str, budget: &mut Budget) -> Result<Regex, String> {
    let cached = CACHE
        .lock()
        .expect("the regex cache is never poisoned")
        .get(pattern);
    let compiled = match cached {
        Some(compiled) => compiled,
        None => {
            let compiled = Arc::new(build(pattern, budget)?);
            CACHE
                .lock()
                .expect("the regex cache is never poisoned")
                .insert(&compiled);
            compiled
        }
    };
    if let Some(remaining) = budget.remaining {
        if compiled.size > remaining {
            budget.use_up();
            return Err(too_large(remaining));
        }
        budget.remaining = Some(remaining - compiled.size);
    }
    Ok(Regex(compiled))
}

/// Build `pattern`, refusing it once its NFA alone outgrows `budget`.
fn build(pattern: &str, budget: &mut Budget) -> Result<Compiled, String> {
    let config = meta::Config::new()
        .nfa_size_limit(budget.remaining)
        .dfa_size_limit(Some(DFA_SIZE_LIMIT))
        .hybrid_cache_capacity(HYBRID_CACHE_CAPACITY);
    let regex = meta::Builder::new()
        .configure(config)
        .build(pattern)
        .map_err(|e| build_error(&e, budget))?;
    // The engine reports nothing for a pattern it searches as a literal, so
    // the text and the value holding it count too, and no pattern is free.
    let size = regex.memory_usage() + pattern.len() + std::mem::size_of::<Compiled>();
    Ok(Compiled {
        pattern: pattern.to_owned(),
        regex,
        size,
    })
}

fn build_error(e: &BuildError, budget: &mut Budget) -> String {
    if let Some(syntax) = e.syntax_error() {
        return format!("not a valid regular expression: {syntax}");
    }
    if e.size_limit().is_some()
        && let Some(left) = budget.use_up()
    {
        return too_large(left);
    }
    format!("not a valid regular expression: {e}")
}

fn too_large(remaining: usize) -> String {
    format!(
        "the pattern compiles to more than the {remaining} bytes left of the regex compiled size limit"
    )
}

// spec: REX#compiling-a-pattern
pub(super) fn register(module: &mut rhai::Module) {
    // Each call counts toward the evaluation's budget, so none is folded.
    FuncRegistration::new("regex")
        .with_volatility(true)
        .set_into_module(
            module,
            |ctx: NativeCallContext, pattern: Dynamic| -> Result<Regex, Box<EvalAltResult>> {
                let pattern = take_string("pattern", pattern)?;
                with_evaluation_budget(&ctx, |budget| compile(&pattern, budget))?
                    .map_err(|e| format!("`pattern`: {e}").into())
            },
        );
}

impl CustomType for Regex {
    fn build(mut builder: TypeBuilder<Self>) {
        builder.with_name(Self::NAME);

        // spec: REX#methods
        builder
            .with_fn(
                "is_match",
                |this: &mut Self, text: Dynamic| -> Result<bool, Box<EvalAltResult>> {
                    let text = take_string("text", text)?;
                    Ok(this.0.regex.is_match(&text))
                },
            )
            .with_fn(
                "find",
                |this: &mut Self, text: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> {
                    let text = take_string("text", text)?;
                    let mut positions = Positions::new(&text);
                    Ok(this.0.regex.find(&text).map_or(Dynamic::UNIT, |m| {
                        match_map(&text, &mut positions, m.start(), m.end())
                    }))
                },
            )
            .with_fn(
                "find_all",
                |ctx: NativeCallContext,
                 this: &mut Self,
                 text: Dynamic|
                 -> Result<rhai::Array, Box<EvalAltResult>> {
                    let text = take_string("text", text)?;
                    let mut positions = Positions::new(&text);
                    bounded(
                        &ctx,
                        this.0
                            .regex
                            .find_iter(&text)
                            .map(|m| match_map(&text, &mut positions, m.start(), m.end())),
                    )
                },
            )
            .with_fn(
                "captures",
                |this: &mut Self, text: Dynamic| -> Result<Dynamic, Box<EvalAltResult>> {
                    let text = take_string("text", text)?;
                    let mut positions = Positions::new(&text);
                    let mut caps = this.0.regex.create_captures();
                    this.0.regex.captures(&text, &mut caps);
                    Ok(if caps.is_match() {
                        captures_map(&text, &mut positions, &caps)
                    } else {
                        Dynamic::UNIT
                    })
                },
            )
            .with_fn(
                "captures_all",
                |ctx: NativeCallContext,
                 this: &mut Self,
                 text: Dynamic|
                 -> Result<rhai::Array, Box<EvalAltResult>> {
                    let text = take_string("text", text)?;
                    let mut positions = Positions::new(&text);
                    bounded(
                        &ctx,
                        this.0
                            .regex
                            .captures_iter(&text)
                            .map(|caps| captures_map(&text, &mut positions, &caps)),
                    )
                },
            )
            .with_fn(
                "replace",
                |ctx: NativeCallContext,
                 this: &mut Self,
                 text: Dynamic,
                 replacement: Dynamic|
                 -> Result<String, Box<EvalAltResult>> {
                    replace(&ctx, this, text, replacement, "replace", Some(1))
                },
            )
            .with_fn(
                "replace_all",
                |ctx: NativeCallContext,
                 this: &mut Self,
                 text: Dynamic,
                 replacement: Dynamic|
                 -> Result<String, Box<EvalAltResult>> {
                    replace(&ctx, this, text, replacement, "replace_all", None)
                },
            )
            .with_fn(
                "split",
                |ctx: NativeCallContext,
                 this: &mut Self,
                 text: Dynamic|
                 -> Result<rhai::Array, Box<EvalAltResult>> {
                    let text = take_string("text", text)?;
                    bounded(
                        &ctx,
                        this.0
                            .regex
                            .split(&text)
                            .map(|span| text[span.range()].into()),
                    )
                },
            )
            .with_fn(
                "split",
                |ctx: NativeCallContext,
                 this: &mut Self,
                 text: Dynamic,
                 limit: Dynamic|
                 -> Result<rhai::Array, Box<EvalAltResult>> {
                    let text = take_string("text", text)?;
                    let limit = limit.as_int().map_err(|t| -> Box<EvalAltResult> {
                        format!("`limit` must be an integer, got {t}").into()
                    })?;
                    let limit = usize::try_from(limit).ok().filter(|&n| n >= 1).ok_or_else(
                        || -> Box<EvalAltResult> {
                            format!("`limit` must be at least 1, got {limit}").into()
                        },
                    )?;
                    bounded(
                        &ctx,
                        this.0
                            .regex
                            .splitn(&text, limit)
                            .map(|span| text[span.range()].into()),
                    )
                },
            );

        // spec: REX#compiling-a-pattern
        builder
            .on_print(|this: &mut Self| this.to_string())
            .on_debug(|this: &mut Self| this.to_string());
    }
}

// spec: REX#positions
/// Character positions in one text, found moving forward from the last one
/// asked for, so a method that walks the text counts each character once.
#[derive(Clone)]
struct Positions<'t> {
    text: &'t str,
    byte: usize,
    chars: usize,
}

impl<'t> Positions<'t> {
    fn new(text: &'t str) -> Self {
        Self {
            text,
            byte: 0,
            chars: 0,
        }
    }

    /// The position of the character at `byte`, which is at or after the
    /// last byte asked for.
    fn at(&mut self, byte: usize) -> INT {
        self.chars += self.text[self.byte..byte].chars().count();
        self.byte = byte;
        INT::try_from(self.chars).expect("a string's length fits an integer")
    }
}

// spec: REX#matches
fn match_map(text: &str, positions: &mut Positions, start: usize, end: usize) -> Dynamic {
    let mut map = rhai::Map::new();
    map.insert("text".into(), text[start..end].into());
    map.insert("start".into(), positions.at(start).into());
    let mut from_start = positions.clone();
    map.insert("end".into(), from_start.at(end).into());
    map.into()
}

// spec: REX#captures
fn captures_map(text: &str, positions: &mut Positions, caps: &Captures) -> Dynamic {
    let whole = caps
        .get_match()
        .expect("only a match's captures are mapped");
    positions.at(whole.start());
    let groups: rhai::Array = (0..caps.group_len())
        .map(|i| {
            caps.get_group(i).map_or(Dynamic::UNIT, |span| {
                // A group can start before an earlier group ends, so each is
                // counted from the start of the whole match.
                match_map(text, &mut positions.clone(), span.start, span.end)
            })
        })
        .collect();
    let mut named = rhai::Map::new();
    for (i, name) in caps.group_info().pattern_names(PatternID::ZERO).enumerate() {
        if let Some(name) = name {
            named.insert(name.into(), groups[i].clone());
        }
    }
    let mut map = rhai::Map::new();
    map.insert("groups".into(), groups.into());
    map.insert("named".into(), named.into());
    map.into()
}

// spec: REX#replacements
enum Replacement {
    Template(Vec<Piece>),
    Closure(FnPtr),
}

/// A part of a replacement string, its group references resolved.
#[derive(Debug, PartialEq)]
enum Piece {
    Literal(String),
    Group(usize),
}

/// A part of a replacement string as written.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Written<'s> {
    Literal(&'s str),
    Dollar,
    Ref(&'s str),
}

fn replace(
    ctx: &NativeCallContext,
    this: &Regex,
    text: Dynamic,
    replacement: Dynamic,
    method: &str,
    limit: Option<usize>,
) -> Result<String, Box<EvalAltResult>> {
    let text = take_string("text", text)?;
    let replacement = take_replacement(this, method, replacement)?;
    let limit = limit.unwrap_or(usize::MAX);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    // A replacement that inserts no group needs only where each match is.
    if let Replacement::Template(pieces) = &replacement
        && let Some(literal) = literal_of(pieces)
    {
        for m in this.0.regex.find_iter(&text).take(limit) {
            out.push_str(&text[last..m.start()]);
            out.push_str(&literal);
            last = m.end();
            // spec: LIM#limits
            within(out.len(), max_string(ctx), STRING)?;
        }
        out.push_str(&text[last..]);
        within(out.len(), max_string(ctx), STRING)?;
        return Ok(out);
    }
    let mut positions = Positions::new(&text);
    let matches = this.0.regex.captures_iter(&text);
    for caps in matches.take(limit) {
        let whole = caps.get_match().expect("the iterator yields matches only");
        out.push_str(&text[last..whole.start()]);
        match &replacement {
            Replacement::Template(pieces) => {
                for piece in pieces {
                    match piece {
                        Piece::Literal(literal) => out.push_str(literal),
                        Piece::Group(i) => {
                            if let Some(span) = caps.get_group(*i) {
                                out.push_str(&text[span.range()]);
                            }
                        }
                    }
                }
            }
            Replacement::Closure(closure) => {
                let captures = captures_map(&text, &mut positions, &caps);
                let replaced = closure.call_within_context::<Dynamic>(ctx, (captures,))?;
                let replaced = replaced.into_string().map_err(|t| -> Box<EvalAltResult> {
                    format!("`{method}`: the replacement closure must return a string, got {t}")
                        .into()
                })?;
                out.push_str(&replaced);
            }
        }
        last = whole.end();
        // spec: LIM#limits
        within(out.len(), max_string(ctx), STRING)?;
    }
    out.push_str(&text[last..]);
    within(out.len(), max_string(ctx), STRING)?;
    Ok(out)
}

/// The text a replacement string inserts, where it inserts no group.
fn literal_of(pieces: &[Piece]) -> Option<String> {
    pieces
        .iter()
        .map(|piece| match piece {
            Piece::Literal(literal) => Some(literal.as_str()),
            Piece::Group(_) => None,
        })
        .collect()
}

// spec: LIM#limits
/// Collect a method's results, refusing them once they pass the engine's
/// array limit rather than building them all for the engine to refuse.
fn bounded(
    ctx: &NativeCallContext,
    items: impl Iterator<Item = Dynamic>,
) -> Result<rhai::Array, Box<EvalAltResult>> {
    let limit = ctx.engine().max_array_size();
    let mut out = rhai::Array::new();
    for item in items {
        out.push(item);
        within(out.len(), limit, ARRAY)?;
    }
    Ok(out)
}

// spec: REX#replacements
fn take_replacement(
    this: &Regex,
    method: &str,
    replacement: Dynamic,
) -> Result<Replacement, Box<EvalAltResult>> {
    // take: dispatch
    if let Some(closure) = replacement.clone().try_cast::<FnPtr>() {
        return Ok(Replacement::Closure(take_closure(
            method,
            "replacement",
            closure,
        )?));
    }
    let template = replacement
        .into_string()
        .map_err(|t| -> Box<EvalAltResult> {
            format!("`replacement` must be a string or a closure, got {t}").into()
        })?;
    parse_template(this, &template)
        .map(Replacement::Template)
        .map_err(|e| format!("`replacement`: {e}").into())
}

// spec: REX#replacements
/// Parse a replacement string, resolving each group reference against the
/// pattern's groups.
fn parse_template(this: &Regex, template: &str) -> Result<Vec<Piece>, String> {
    let written: Vec<Written> = repeat(0.., written).parse(template).map_err(
        |e: winnow::error::ParseError<&str, winnow::error::ContextError>| {
            format!(
                "the `$` at position {} begins no group reference; `$$` writes a `$`",
                template[..e.offset()].chars().count()
            )
        },
    )?;
    written
        .into_iter()
        .map(|part| match part {
            Written::Literal(literal) => Ok(Piece::Literal(literal.to_owned())),
            Written::Dollar => Ok(Piece::Literal("$".to_owned())),
            Written::Ref(name) => this.group(name).map(Piece::Group).ok_or_else(|| {
                format!("refers to the group {name:?}, which the pattern does not have")
            }),
        })
        .collect()
}

/// One part of a replacement string: text up to the next `$`, `$$`, or a
/// group reference.
fn written<'s>(input: &mut &'s str) -> winnow::Result<Written<'s>> {
    alt((
        take_till(1.., '$').map(Written::Literal),
        "$$".value(Written::Dollar),
        preceded('$', delimited('{', take_till(0.., '}'), '}')).map(Written::Ref),
        preceded(
            '$',
            take_while(1.., |c: char| c.is_ascii_alphanumeric() || c == '_'),
        )
        .map(Written::Ref),
    ))
    .parse_next(input)
}

impl Regex {
    /// The index of the group `name` refers to: its number when it is
    /// written in decimal digits, otherwise its name.
    fn group(&self, name: &str) -> Option<usize> {
        let info = self.0.regex.group_info();
        if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
            return name
                .parse::<usize>()
                .ok()
                .filter(|&i| i < info.group_len(PatternID::ZERO));
        }
        info.to_index(PatternID::ZERO, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiled(pattern: &str, size: usize) -> Arc<Compiled> {
        Arc::new(Compiled {
            pattern: pattern.to_owned(),
            regex: meta::Regex::new(pattern).expect("a valid pattern"),
            size,
        })
    }

    /// A compiled size making a pattern take a third of the cache.
    const THIRD: usize = CACHE_CAPACITY / 3 - SEARCH_ALLOWANCE;

    #[test]
    fn the_cache_evicts_the_least_recently_used_patterns_to_make_room() {
        let mut cache = Cache::default();
        cache.insert(&compiled("a", THIRD));
        cache.insert(&compiled("b", THIRD));
        cache.insert(&compiled("c", THIRD));
        cache.get("a").expect("cached");
        cache.insert(&compiled("d", THIRD));
        assert!(!cache.patterns.contains_key("b"));
        assert!(
            ["a", "c", "d"]
                .iter()
                .all(|p| cache.patterns.contains_key(*p))
        );
        assert_eq!(cache.size, 3 * (THIRD + SEARCH_ALLOWANCE));
    }

    #[test]
    fn a_pattern_larger_than_the_cache_is_not_held() {
        let mut cache = Cache::default();
        cache.insert(&compiled("a", 100));
        cache.insert(&compiled("b", CACHE_CAPACITY));
        assert!(!cache.patterns.contains_key("b"));
        assert!(cache.patterns.contains_key("a"));
    }

    #[test]
    fn caching_a_pattern_twice_counts_it_once() {
        let mut cache = Cache::default();
        cache.insert(&compiled("a", 100));
        cache.insert(&compiled("a", 100));
        assert_eq!(cache.size, 100 + SEARCH_ALLOWANCE);
        assert_eq!(cache.order.len(), 1);
    }
}
