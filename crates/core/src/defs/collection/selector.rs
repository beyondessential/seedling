use std::collections::HashSet;

use rhai::Map;
use wildmatch::WildMatch;

use crate::defs::resource::ResourceKind;

use super::handle::ResourceHandle;

#[derive(Clone, Default)]
pub struct Selector {
    pub types: Option<Vec<ResourceKind>>,
    pub names: Option<HashSet<String>>,
    pub name_patterns: Option<Vec<String>>,
}

impl Selector {
    pub fn from_map(map: &Map) -> Self {
        // l[impl collection.select.types]
        let types = map.get("types").and_then(|v| {
            v.clone().try_cast::<rhai::Array>().map(|arr| {
                arr.into_iter()
                    .filter_map(|item| item.try_cast::<ResourceKind>())
                    .collect()
            })
        });

        // l[impl collection.select.names]
        let names = map.get("names").and_then(|v| {
            v.clone().try_cast::<rhai::Array>().map(|arr| {
                arr.into_iter()
                    .filter_map(|item| item.into_string().ok())
                    .collect()
            })
        });

        // l[impl collection.select.name-patterns]
        let name_patterns = map.get("name_patterns").and_then(|v| {
            v.clone().try_cast::<rhai::Array>().map(|arr| {
                arr.into_iter()
                    .filter_map(|item| item.into_string().ok())
                    .collect()
            })
        });

        Selector {
            types,
            names,
            name_patterns,
        }
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
