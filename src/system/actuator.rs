use std::{collections::BTreeMap, sync::Arc, time::Duration};

use ipnet::Ipv6Net;
use snafu::Snafu;

use crate::{
    defs::{
        enums::OnExit,
        resource::{Resource, ResourceKind},
        service::ServicePort,
    },
    runtime::{identity::ResourceInstance, registry::InstanceRegistry},
    system::{
        System,
        translate::{
            container::{deployment_spec, job_spec, podman_args},
            proxy::{instance_ipv6, pod_network_prefix},
        },
        types::{ActiveState, TransientRestart, TransientUnitSpec},
    },
};

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Actuation-time error. The backend variant is intentionally erased:
/// callers see `ActuateError::Container` but cannot match on internal types.
#[derive(Debug, Snafu)]
pub enum ActuateError {
    #[snafu(display("container backend: {source}"))]
    Container {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("process manager: {source}"))]
    Process {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("proxy: {source}"))]
    Proxy {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("data plane: {source}"))]
    DataPlane {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[snafu(display("image {reference} not found and pull failed"))]
    ImageUnavailable { reference: String },
    #[snafu(display("resource kind {kind:?} is not supported by this actuator"))]
    UnsupportedKind { kind: ResourceKind },
}

// ---------------------------------------------------------------------------
// Naming helpers
// ---------------------------------------------------------------------------

fn pod_network_name(instance: &ResourceInstance) -> String {
    format!("seedling-{}", instance.display_name)
}

fn unit_name(instance: &ResourceInstance) -> String {
    format!("seedling-{}.service", instance.display_name)
}

fn map_on_exit(on_exit: OnExit) -> TransientRestart {
    match on_exit {
        OnExit::Restart => TransientRestart::Always,
        OnExit::Terminate => TransientRestart::No,
        OnExit::RestartOnFailure => TransientRestart::OnFailure,
    }
}

// ---------------------------------------------------------------------------
// Actuator
// ---------------------------------------------------------------------------

pub struct Actuator {
    driver: Arc<System>,
    node_prefix: Ipv6Net,
    registry: Arc<dyn InstanceRegistry>,
}

impl Actuator {
    pub fn new(
        driver: Arc<System>,
        node_prefix: Ipv6Net,
        registry: Arc<dyn InstanceRegistry>,
    ) -> Self {
        Self {
            driver,
            node_prefix,
            registry,
        }
    }

    // r[impl actuate.deployment.start]
    // r[impl actuate.volume.start]
    /// Ensure all primitives for this instance exist and are running.
    pub async fn start(
        &self,
        instance: &ResourceInstance,
        resource: &Resource,
    ) -> Result<Option<String>, ActuateError> {
        match resource {
            Resource::Deployment(dep) => {
                let (image, raw_mounts, restart) = {
                    let def = dep.def.lock();
                    let pod = def.pod.lock();
                    let container = pod.container.lock();
                    let image = container.image.clone().unwrap_or_default();
                    let raw_mounts = pod.service_mounts.clone();
                    let restart = map_on_exit(container.on_exit);
                    (image, raw_mounts, restart)
                };
                self.start_pod_instance(
                    instance,
                    &image,
                    &raw_mounts,
                    restart,
                    |net_name, net_prefix, mounts| {
                        let guard = dep.def.lock();
                        let spec = deployment_spec(
                            &guard,
                            instance,
                            &BTreeMap::new(),
                            &(net_name, net_prefix),
                            mounts,
                        );
                        podman_args(&spec)
                    },
                )
                .await
            }
            Resource::Job(job) => {
                let (image, raw_mounts, restart) = {
                    let def = job.def.lock();
                    let pod = def.pod.lock();
                    let container = pod.container.lock();
                    let image = container.image.clone().unwrap_or_default();
                    let raw_mounts = pod.service_mounts.clone();
                    let restart = map_on_exit(container.on_exit);
                    (image, raw_mounts, restart)
                };
                self.start_pod_instance(
                    instance,
                    &image,
                    &raw_mounts,
                    restart,
                    |net_name, net_prefix, mounts| {
                        let guard = job.def.lock();
                        let spec = job_spec(
                            &guard,
                            instance,
                            &BTreeMap::new(),
                            &(net_name, net_prefix),
                            mounts,
                        );
                        podman_args(&spec)
                    },
                )
                .await
            }
            Resource::Volume(v) => {
                let name = v
                    .name
                    .as_ref()
                    .map(|n| n.as_str())
                    .unwrap_or(&instance.display_name)
                    .to_owned();
                if !self
                    .driver
                    .container
                    .volume_exists(&name)
                    .await
                    .map_err(|e| ActuateError::Container { source: e })?
                {
                    self.driver
                        .container
                        .create_volume(&name)
                        .await
                        .map_err(|e| ActuateError::Container { source: e })?;
                }
                // TODO: Volume writes (populating files into the volume)
                Ok(None)
            }
            Resource::ExternalVolume(_) | Resource::ExternalService(_) => Ok(None),
            Resource::Service(_) | Resource::HttpService(_) => Ok(None),
            Resource::Ingress(_) => Ok(None),
        }
    }

    // r[impl actuate.deployment.stop]
    // r[impl actuate.volume.stop]
    /// Stop and remove all primitives for this instance.
    pub async fn stop(
        &self,
        instance: &ResourceInstance,
        resource: &Resource,
    ) -> Result<(), ActuateError> {
        match resource {
            Resource::Deployment(_) | Resource::Job(_) => self.stop_pod_instance(instance).await,
            Resource::Volume(v) => {
                let name = v
                    .name
                    .as_ref()
                    .map(|n| n.as_str())
                    .unwrap_or(&instance.display_name)
                    .to_owned();
                if self
                    .driver
                    .container
                    .volume_exists(&name)
                    .await
                    .map_err(|e| ActuateError::Container { source: e })?
                {
                    self.driver
                        .container
                        .remove_volume(&name)
                        .await
                        .map_err(|e| ActuateError::Container { source: e })?;
                }
                Ok(())
            }
            Resource::ExternalVolume(_) | Resource::ExternalService(_) => Ok(()),
            Resource::Service(_) | Resource::HttpService(_) => Ok(()),
            Resource::Ingress(_) => Ok(()),
        }
    }

    /// In-place update (e.g. rolling a container to a new image or config).
    pub async fn update(
        &self,
        _instance: &ResourceInstance,
        _old: &Resource,
        _new: &Resource,
    ) -> Result<(), ActuateError> {
        todo!("actuator update: not yet implemented")
    }

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    /// Resolves `service_mounts` declared on a pod to `(mount_port, service_ip,
    /// service_port)` tuples, computing each service's stable IPv6 address from
    /// the node prefix and the service's persisted instance identity.
    fn resolve_service_mounts(
        &self,
        instance: &ResourceInstance,
        mounts: &[ServicePort],
    ) -> Vec<(u16, std::net::Ipv6Addr, u16)> {
        mounts
            .iter()
            .map(|sp| {
                let svc_instance = self.registry.get_or_create_singleton(
                    &instance.app,
                    ResourceKind::Service,
                    Some(sp.service.name.as_str()),
                );
                let service_ip = instance_ipv6(&self.node_prefix, &svc_instance);
                (sp.port, service_ip, sp.port)
            })
            .collect()
    }

    async fn start_pod_instance(
        &self,
        instance: &ResourceInstance,
        image: &str,
        raw_mounts: &[ServicePort],
        restart: TransientRestart,
        build_argv: impl FnOnce(String, Ipv6Net, &[(u16, std::net::Ipv6Addr, u16)]) -> Vec<String>,
    ) -> Result<Option<String>, ActuateError> {
        // Ensure the container image is available.
        if !self
            .driver
            .container
            .image_exists(image)
            .await
            .map_err(|e| ActuateError::Container { source: e })?
        {
            self.driver.container.pull_image(image).await.map_err(|_| {
                ActuateError::ImageUnavailable {
                    reference: image.to_owned(),
                }
            })?;
        }

        // Ensure the pod network exists.
        let net_name = pod_network_name(instance);
        let net_prefix = pod_network_prefix(&self.node_prefix, instance);
        let bridge_name = if !self
            .driver
            .container
            .network_exists(&net_name)
            .await
            .map_err(|e| ActuateError::Container { source: e })?
        {
            Some(
                self.driver
                    .container
                    .create_network(&net_name, net_prefix)
                    .await
                    .map_err(|e| ActuateError::Container { source: e })?,
            )
        } else {
            None
        };

        // Skip if the unit is already active.
        let unit = unit_name(instance);
        if let Some(state) = self
            .driver
            .process
            .unit_state(&unit)
            .await
            .map_err(|e| ActuateError::Process { source: e })?
            && matches!(state.active, ActiveState::Active | ActiveState::Activating)
        {
            return Ok(bridge_name);
        }

        // Resolve service mounts and build the argv.
        let mounts = self.resolve_service_mounts(instance, raw_mounts);
        let argv = build_argv(net_name, net_prefix, &mounts);

        self.driver
            .process
            .start_transient(TransientUnitSpec {
                name: unit,
                description: format!("seedling container {}", instance.display_name),
                exec_start: argv,
                restart,
            })
            .await
            .map_err(|e| ActuateError::Process { source: e })?;

        Ok(bridge_name)
    }

    async fn stop_pod_instance(&self, instance: &ResourceInstance) -> Result<(), ActuateError> {
        let unit = unit_name(instance);

        // Stop the unit if it exists, then wait for it to terminate.
        if self
            .driver
            .process
            .unit_state(&unit)
            .await
            .map_err(|e| ActuateError::Process { source: e })?
            .is_some()
        {
            self.driver
                .process
                .stop_unit(&unit)
                .await
                .map_err(|e| ActuateError::Process { source: e })?;
            self.driver
                .process
                .wait_unit_stopped(&unit, Duration::from_secs(30))
                .await
                .map_err(|e| ActuateError::Process { source: e })?;
        }

        // Force-remove the container in case it outlived the unit.
        self.driver
            .container
            .remove_container(&instance.display_name, true)
            .await
            .map_err(|e| ActuateError::Container { source: e })?;

        // Remove the pod network.
        let net_name = pod_network_name(instance);
        if self
            .driver
            .container
            .network_exists(&net_name)
            .await
            .map_err(|e| ActuateError::Container { source: e })?
        {
            self.driver
                .container
                .remove_network(&net_name)
                .await
                .map_err(|e| ActuateError::Container { source: e })?;
        }

        Ok(())
    }
}
