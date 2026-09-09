use std::collections::HashSet;

use rhai::{EvalAltResult, Map};
use wildmatch::WildMatch;

use crate::defs::resource::ResourceKind;
use crate::defs::take::{take_array_of, take_string_array};

use super::handle::ResourceHandle;

#[derive(Clone, Default)]
pub struct Selector {
    pub types: Option<Vec<ResourceKind>>,
    pub names: Option<HashSet<String>>,
    pub name_patterns: Option<Vec<String>>,
}

impl Selector {
    /// Build a selector from a `select` criterion map, throwing on anything
    /// the spec does not define.
    ///
    /// Silently dropping a malformed criterion is the worst available
    /// outcome: a `select` with no surviving criteria matches *every*
    /// resource in the app, so `select(#{ types: ResourceType.Service })` —
    /// array brackets forgotten — turns a following `rt.stop` into a stop of
    /// every workload. Failing at the `select` call names the line instead.
    // l[impl collection.select]
    pub fn from_map(map: &Map) -> Result<Self, Box<EvalAltResult>> {
        // The spec says all possible keys are defined in it, so an unknown
        // key is a typo — and a typo'd criterion is a criterion that does not
        // constrain anything.
        for key in map.keys() {
            if !matches!(key.as_str(), "types" | "names" | "name_patterns") {
                return Err(format!(
                    "unknown select criterion `{key}`; expected one of \
                     `types`, `names`, `name_patterns`"
                )
                .into());
            }
        }

        // l[impl collection.select.types]
        let types = map
            .get("types")
            .map(|v| take_array_of::<ResourceKind>("select types", v.clone()))
            .transpose()?;

        // l[impl collection.select.names]
        let names = map
            .get("names")
            .map(|v| {
                take_string_array("select names", v.clone())
                    .map(|names| names.into_iter().collect::<HashSet<_>>())
            })
            .transpose()?;

        // l[impl collection.select.name-patterns]
        let name_patterns = map
            .get("name_patterns")
            .map(|v| take_string_array("select name_patterns", v.clone()))
            .transpose()?;

        Ok(Selector {
            types,
            names,
            name_patterns,
        })
    }

    pub fn matches(&self, handle: &ResourceHandle) -> bool {
        // l[impl collection.select.types]
        if let Some(types) = &self.types
            && !types.contains(&handle.kind())
        {
            return false;
        }

        // l[impl collection.select.names]
        if let Some(names) = &self.names
            && !names.contains(handle.name())
        {
            return false;
        }

        // l[impl collection.select.name-patterns]
        if let Some(patterns) = &self.name_patterns
            && !patterns
                .iter()
                .any(|p| WildMatch::new(p).matches(handle.name()))
        {
            return false;
        }

        true
    }
}
