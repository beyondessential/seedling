use crate::{
    defs::resource::Resource,
    runtime::{
        barrier::oracle::derive_lifecycle_state,
        db::Db,
        history::{WorldObservation, query_observations},
        identity::ResourceInstance,
        lifecycle::LifecycleState,
    },
};

/// The state of an instance's health check, as `/apps/show` reports it.
// spec: APPS#resources
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceHealth {
    Starting,
    Passing,
    Failing,
}

impl InstanceHealth {
    pub fn name(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Passing => "passing",
            Self::Failing => "failing",
        }
    }
}

/// Whether the resource is a Deployment that declares a health check.
pub fn declares_healthcheck(resource: &Resource) -> bool {
    let Resource::Deployment(dep) = resource else {
        return false;
    };
    let dep_def = dep.def.lock();
    let pod_def = dep_def.pod.lock();
    let container = pod_def.container.lock();
    container.healthcheck.is_some()
}

/// The health of an instance of a Deployment that declares a health check,
/// from its observations in recorded order.
///
/// It is `None` without a running container and until a check has been
/// observed since the container started.
// spec: APPS#resources
pub fn derive_health(
    instance: &ResourceInstance,
    observations: &[WorldObservation],
) -> Option<InstanceHealth> {
    if !matches!(
        derive_lifecycle_state(instance, observations),
        LifecycleState::Running | LifecycleState::Ready
    ) {
        return None;
    }
    let mut health = None;
    for obs in observations {
        match obs.obs_kind.as_str() {
            "container_created" | "container_exited" | "container_removed" => health = None,
            "health_check_starting" => health = Some(InstanceHealth::Starting),
            "health_check_pass" => health = Some(InstanceHealth::Passing),
            "health_check_fail" => health = Some(InstanceHealth::Failing),
            _ => {}
        }
    }
    health
}

/// An instance's health, or an error where its observations cannot be read,
/// which is not the same as no health.
// spec: APPS#resources
pub fn read_health(
    db: &Db,
    instance: &ResourceInstance,
    declares_healthcheck: bool,
) -> rusqlite::Result<Option<InstanceHealth>> {
    if !declares_healthcheck {
        return Ok(None);
    }
    let observations = query_observations(db, instance)?;
    Ok(derive_health(instance, &observations))
}

#[cfg(test)]
mod tests {
    use seedling_protocol::names::AppName;

    use super::*;
    use crate::defs::resource::ResourceKind;

    fn instance() -> ResourceInstance {
        ResourceInstance::new_singleton(
            AppName::new_unchecked("demo"),
            ResourceKind::Deployment,
            "web",
        )
    }

    fn history(kinds: &[&str]) -> Vec<WorldObservation> {
        kinds
            .iter()
            .enumerate()
            .map(|(i, kind)| WorldObservation {
                id: i as i64,
                recorded_at: i as i64,
                resource: instance(),
                obs_kind: (*kind).to_owned(),
                payload: serde_json::json!({}),
            })
            .collect()
    }

    // spec: APPS#resources
    #[test]
    fn health_follows_the_latest_check_of_the_running_container() {
        let health = |kinds: &[&str]| derive_health(&instance(), &history(kinds));
        let up = ["container_created", "container_running"];
        fn with<'a>(up: &[&'a str], extra: &[&'a str]) -> Vec<&'a str> {
            [up, extra].concat()
        }

        assert_eq!(health(&with(&up, &[])), None);
        assert_eq!(
            health(&with(&up, &["health_check_starting"])),
            Some(InstanceHealth::Starting)
        );
        assert_eq!(
            health(&with(&up, &["health_check_starting", "health_check_pass"])),
            Some(InstanceHealth::Passing)
        );
        assert_eq!(
            health(&with(&up, &["health_check_pass", "health_check_fail"])),
            Some(InstanceHealth::Failing)
        );
        assert_eq!(
            health(&with(
                &up,
                &[
                    "health_check_pass",
                    "health_check_fail",
                    "health_check_pass"
                ]
            )),
            Some(InstanceHealth::Passing)
        );
    }

    // spec: APPS#resources
    #[test]
    fn health_is_none_without_a_running_container_or_since_a_new_one_started() {
        let health = |kinds: &[&str]| derive_health(&instance(), &history(kinds));
        assert_eq!(health(&[]), None);
        assert_eq!(
            health(&["container_created", "health_check_starting"]),
            None,
            "not running yet"
        );
        assert_eq!(
            health(&[
                "container_created",
                "container_running",
                "health_check_pass",
                "container_exited",
            ]),
            None
        );
        assert_eq!(
            health(&[
                "container_created",
                "container_running",
                "health_check_pass",
                "container_exited",
                "container_removed",
                "container_created",
                "container_running",
            ]),
            None,
            "the earlier container's check is not the new one's"
        );
    }
}
