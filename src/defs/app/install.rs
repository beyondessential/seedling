use std::{collections::BTreeMap, str::FromStr as _};

use rhai::{EvalAltResult, FnPtr, Map, TypeBuilder};

use super::super::install::{InstallDef, InstallRequirementDef, InstallRequirementKind};
use super::App;

pub(super) fn on_app(builder: &mut TypeBuilder<App>) {
    // l[impl action.install]
    builder
        .with_fn("on_install", |this: &mut App, closure: FnPtr| {
            this.def.lock().install = Some(InstallDef {
                requirements: BTreeMap::new(),
            });
            super::capture_install(closure);
        })
        .with_fn(
            "on_install",
            |this: &mut App, closure: FnPtr, requirements: Map| -> Result<(), Box<EvalAltResult>> {
                let reqs = parse_install_requirements(&requirements)?;
                this.def.lock().install = Some(InstallDef { requirements: reqs });
                super::capture_install(closure);
                Ok(())
            },
        );
}

// l[impl action.install.requirements.kind-unknown]
fn parse_install_requirements(
    map: &Map,
) -> Result<BTreeMap<String, InstallRequirementDef>, Box<EvalAltResult>> {
    let mut reqs = BTreeMap::new();
    for (key, value) in map {
        if let Some(req_map) = value.read_lock::<Map>() {
            let kind = match req_map
                .get("kind")
                .and_then(|v| v.clone().into_string().ok())
            {
                Some(s) => InstallRequirementKind::from_str(&s).map_err(|_| {
                    Box::<EvalAltResult>::from(format!("unknown install requirement kind: \"{s}\""))
                })?,
                None => InstallRequirementKind::default(),
            };

            let required = req_map
                .get("required")
                .and_then(|v| v.as_bool().ok())
                .unwrap_or(true);

            let default_value = req_map
                .get("default_value")
                .and_then(|v| v.clone().into_string().ok());

            let description = req_map
                .get("description")
                .and_then(|v| v.clone().into_string().ok());

            reqs.insert(
                key.to_string(),
                InstallRequirementDef {
                    kind,
                    required,
                    default_value,
                    description,
                },
            );
        }
    }
    Ok(reqs)
}
