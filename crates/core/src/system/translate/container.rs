use std::{
    collections::{BTreeMap, HashMap},
    net::{IpAddr, Ipv6Addr},
    path::Path,
};

use ipnet::Ipv6Net;
use seedling_protocol::names::ExternalVolumeName;

use std::time::Duration;

use crate::{
    defs::{
        container::{HealthcheckKind, VolumeMount},
        deployment::DeploymentDef,
        job::JobDef,
        pod::PodDef,
    },
    runtime::identity::{ResourceInstance, VolumeName},
    system::{
        translate::proxy::node_mount_addr,
        types::{ContainerSpec, HealthCheckSpec, Mount, MountSource, ResolvedExternalMount},
    },
};

/// Builds a `ContainerSpec` for one instance of a `Deployment`.
#[expect(
    clippy::too_many_arguments,
    reason = "spec building requires all context"
)]
pub fn deployment_spec(
    def: &DeploymentDef,
    instance: &ResourceInstance,
    _params: &BTreeMap<String, String>,
    network: &(String, Ipv6Net),
    mounts: &[(u16, Ipv6Addr, u16)],
    dns_servers: &[Ipv6Addr],
    volumes_dir: Option<&Path>,
    external_volumes: &HashMap<ExternalVolumeName, ResolvedExternalMount>,
    restart_gen: u64,
) -> ContainerSpec {
    let pod = def.pod.lock();
    let mut spec = spec_from_pod(
        &pod,
        instance,
        network,
        mounts,
        dns_servers,
        volumes_dir,
        external_volumes,
    );
    // i[impl deployment.restart]
    if restart_gen > 0 {
        spec.labels
            .insert("seedling.restart-gen".to_string(), restart_gen.to_string());
    }
    spec
}

/// Builds a `ContainerSpec` for a `Job` instance.
#[expect(
    clippy::too_many_arguments,
    reason = "spec building requires all context"
)]
pub fn job_spec(
    def: &JobDef,
    instance: &ResourceInstance,
    _params: &BTreeMap<String, String>,
    network: &(String, Ipv6Net),
    mounts: &[(u16, Ipv6Addr, u16)],
    dns_servers: &[Ipv6Addr],
    volumes_dir: Option<&Path>,
    external_volumes: &HashMap<ExternalVolumeName, ResolvedExternalMount>,
    restart_gen: u64,
) -> ContainerSpec {
    let pod = def.pod.lock();
    let mut spec = spec_from_pod(
        &pod,
        instance,
        network,
        mounts,
        dns_servers,
        volumes_dir,
        external_volumes,
    );
    if restart_gen > 0 {
        spec.labels
            .insert("seedling.restart-gen".to_string(), restart_gen.to_string());
    }
    spec
}

/// Produces the `podman run [...]` argv from a `ContainerSpec`.
///
/// This is the `ExecStart` value for the transient systemd unit. The returned
/// vec begins with `["podman", "run", "--rm", ...]` and ends with the image
/// reference followed by any command arguments.
pub fn podman_args(spec: &ContainerSpec) -> Vec<String> {
    let mut args = vec!["podman".to_string(), "run".to_string(), "--rm".to_string()];

    // r[impl actuate.container.journal-metadata]
    // Disable podman's built-in journald log driver; container stdout/stderr flows
    // through podman's own process stdout, which systemd captures once. Without this,
    // both podman's journald driver and systemd's unit capture write the same lines.
    args.push("--log-driver=none".to_string());

    // r[impl actuate.container.hardening]
    args.push("--cap-drop=ALL".to_string());
    for cap in &spec.extra_caps {
        args.push(format!("--cap-add={cap}"));
    }
    args.push("--security-opt".to_string());
    args.push("no-new-privileges".to_string());
    if !spec.writable_rootfs {
        args.push("--read-only".to_string());
        args.push("--tmpfs".to_string());
        args.push("/tmp".to_string());
    }
    args.push("--pids-limit".to_string());
    args.push(spec.pids_limit.to_string());
    args.push("--ulimit".to_string());
    args.push("nofile=65536:65536".to_string());
    if let Some(mem) = &spec.memory {
        args.push(format!("--memory={mem}"));
    }
    if let Some(cpus) = spec.cpus {
        args.push(format!("--cpus={cpus}"));
    }
    if let Some(workdir) = &spec.workdir {
        args.push(format!("--workdir={workdir}"));
    }

    args.push("--name".to_string());
    args.push(spec.name.clone());

    args.push("--network".to_string());
    args.push(spec.network.clone());

    for var in &spec.env {
        args.push("--env".to_string());
        args.push(var.to_string());
    }

    for mount in &spec.mounts {
        match &mount.source {
            MountSource::Tmpfs => {
                args.push("--mount".to_string());
                args.push(format!("type=tmpfs,destination={}", mount.target));
            }
            MountSource::Volume(name) => {
                let mut s = format!("{name}:{}", mount.target);
                if mount.read_only {
                    s.push_str(":ro");
                }
                args.push("--volume".to_string());
                args.push(s);
            }
            MountSource::Bind(path) => {
                let mut s = format!("{}:{}", path.display(), mount.target);
                if mount.read_only {
                    s.push_str(":ro");
                }
                args.push("--volume".to_string());
                args.push(s);
            }
        }
    }

    for (k, v) in &spec.labels {
        args.push("--label".to_string());
        args.push(format!("{k}={v}"));
    }

    for (host, ip) in &spec.hosts {
        args.push("--add-host".to_string());
        args.push(format!("{host}:{ip}"));
    }

    // r[impl infra.pod.dns]
    for dns in &spec.dns_servers {
        args.push("--dns".to_string());
        args.push(dns.to_string());
    }

    // l[impl deployment.healthcheck]
    if let Some(health) = &spec.health {
        args.push("--health-cmd".to_string());
        // Podman accepts a shell command string or a JSON array for health checks.
        args.push(
            serde_json::to_string(&health.command).unwrap_or_else(|_| health.command.join(" ")),
        );
        args.push("--health-interval".to_string());
        args.push(format!("{}s", health.interval.as_secs()));
        args.push("--health-timeout".to_string());
        args.push(format!("{}s", health.timeout.as_secs()));
        args.push("--health-retries".to_string());
        args.push(health.retries.to_string());
        args.push("--health-start-period".to_string());
        args.push(format!("{}s", health.start_period.as_secs()));
        // r[impl autonomous.healthcheck-replace]
        // Seedling owns the response to a failing healthcheck (replace flow).
        // Tell podman to never act on its own — never kill, never restart.
        args.push("--health-on-failure".to_string());
        args.push("none".to_string());
    }

    if !spec.entrypoint.is_empty() {
        // Clear the image's ENTRYPOINT so the image's baked-in CMD is not
        // appended to our command (Kubernetes `command:` semantics: when an
        // entrypoint override is present, the image's CMD is suppressed).
        // Our entrypoint args are then folded into the positional CMD args,
        // which also override the image CMD.
        args.push("--entrypoint".to_string());
        args.push("[]".to_string());
    }

    args.push(spec.image.clone());
    // Entrypoint args precede any explicit cmd (arg) overrides.
    args.extend(spec.entrypoint.iter().cloned());
    args.extend(spec.command.iter().cloned());

    args
}

/// Compute a SHA-256 hash of the canonical podman argv for a container spec,
/// excluding the `seedling.spec-hash` label itself (which would be circular).
/// The hash covers the complete desired container configuration: image, command,
/// args, env, mounts, health, hosts, and all other labels.
pub fn spec_hash(spec: &ContainerSpec) -> String {
    use sha2::{Digest, Sha256};
    let mut hashable = spec.clone();
    hashable.labels.remove("seedling.spec-hash");
    let args = podman_args(&hashable);
    let digest = Sha256::digest(args.join("\x00").as_bytes());
    use std::fmt::Write as FmtWrite;
    let mut hex = String::with_capacity(64);
    for b in digest.iter() {
        write!(hex, "{b:02x}").expect("write to String is infallible");
    }
    hex
}

// ---------------------------------------------------------------------------
// Shared pod → spec logic
// ---------------------------------------------------------------------------

fn spec_from_pod(
    pod: &PodDef,
    instance: &ResourceInstance,
    network: &(String, Ipv6Net),
    mounts: &[(u16, Ipv6Addr, u16)],
    dns_servers: &[Ipv6Addr],
    volumes_dir: Option<&Path>,
    external_volumes: &HashMap<ExternalVolumeName, ResolvedExternalMount>,
) -> ContainerSpec {
    let container = pod.container.lock();

    let image = container.image.clone().unwrap_or_default();

    // BSL `.command()` overrides the container entrypoint (passed as
    // `--entrypoint` to podman run).  BSL `.arg()` provides the CMD arguments
    // that follow the image name.  Keeping them separate matches the OCI model
    // and lets shell sessions override the entrypoint without affecting any
    // default CMD args.
    let entrypoint = container.command.clone().unwrap_or_default();
    let command = container.args.clone().unwrap_or_default();

    let env = container.env.clone();

    let sys_mounts = container
        .volume_mounts
        .iter()
        .map(|(path, vm)| {
            if let VolumeMount::ExternalVolume(ev) = vm {
                // l[impl volume.external.dynamic]
                // Operation-scoped binding takes precedence over the static mapping table.
                if let Some(binding) = &ev.operation_binding {
                    return Mount {
                        source: MountSource::Bind(binding.host_path.clone()),
                        target: path.to_string_lossy().into_owned(),
                        read_only: binding.read_only,
                    };
                }
                // Fall back to static external volume mappings.
                if let Some(resolved) = external_volumes.get(ev.name.as_str()) {
                    return Mount {
                        source: resolved.source.clone(),
                        target: path.to_string_lossy().into_owned(),
                        read_only: resolved.read_only,
                    };
                }
            }

            let source = match vm {
                VolumeMount::Volume(v) => {
                    let name = match &v.name {
                        // Named volumes use the canonical Volume-resource
                        // display name so the bind-mount source matches the
                        // path the Volume actuator creates.
                        Some(n) => VolumeName::for_app(instance.app.as_str(), n.as_str())
                            .as_str()
                            .to_owned(),
                        None => v
                            .anon_id
                            .clone()
                            .expect("anonymous volume must have an anon_id"),
                    };
                    // r[impl actuate.volume.storage]
                    let is_named = v.name.is_some();
                    let tmpfs = v.def.lock().tmpfs;
                    if tmpfs {
                        // r[impl actuate.volume.tmpfs]
                        // Tmpfs volumes (named or anonymous) are bind-mounted
                        // from a tmpfs-backed host directory under /run.
                        // Keep the path in sync with the actuator's
                        // `TMPFS_VOLUMES_DIR`.
                        MountSource::Bind(
                            std::path::PathBuf::from(crate::system::actuator::TMPFS_VOLUMES_DIR)
                                .join(&name),
                        )
                    } else if is_named && let Some(dir) = volumes_dir {
                        MountSource::Bind(dir.join(&name))
                    } else {
                        MountSource::Volume(name)
                    }
                }
                VolumeMount::ExternalVolume(ev) => {
                    // TODO: ExternalVolume is external to this BSL app but still within
                    // seedling. The name here is a BSL-level reference, not yet a resolved
                    // podman volume name. When cross-app volume sharing is implemented,
                    // this must resolve to the source volume's display_name
                    // (e.g. "{source_app}-{vol_name}") using seedling's instance registry.
                    MountSource::Volume(ev.name.to_string())
                }
            };
            Mount {
                source,
                target: path.to_string_lossy().into_owned(),
                read_only: false,
            }
        })
        .collect();

    let mut labels = BTreeMap::new();
    labels.insert("seedling.app".to_string(), instance.app.as_str().to_owned());
    labels.insert("seedling.instance".to_string(), instance.id.to_hex());
    labels.insert("seedling.kind".to_string(), format!("{:?}", instance.kind));
    labels.insert(
        "seedling.display-name".to_string(),
        instance.display_name.clone(),
    );
    // Changing this value forces a spec-hash change and container restart,
    // ensuring all running containers pick up new systemd unit properties.
    labels.insert("seedling.unit-gen".to_string(), "1".to_string());

    // Inject the mount endpoint host entry so that containers can reach
    // mounted services via `localmount:<port>`. The ::2 address lives on
    // the host side of the pod bridge.
    let hosts = if mounts.is_empty() {
        vec![]
    } else {
        let mount_endpoint = node_mount_addr(&network.1);
        vec![("localmount".to_string(), IpAddr::V6(mount_endpoint))]
    };

    let health = container.healthcheck.as_ref().map(|hc| {
        let HealthcheckKind::Command { cmd } = &hc.kind;
        HealthCheckSpec {
            command: cmd.clone(),
            interval: Duration::from_secs(hc.interval_secs),
            timeout: Duration::from_secs(hc.timeout_secs),
            retries: hc.retries,
            start_period: Duration::from_secs(hc.start_period_secs),
        }
    });

    let mut spec = ContainerSpec {
        name: instance.display_name.clone(),
        image,
        command,
        entrypoint,
        env,
        mounts: sys_mounts,
        network: network.0.clone(),
        labels,
        health,
        hosts,
        dns_servers: dns_servers.to_vec(),
        memory: container.memory.clone(),
        cpus: container.cpus,
        extra_caps: container.extra_caps.clone(),
        writable_rootfs: container.writable_rootfs,
        pids_limit: container.pids_limit.unwrap_or(256),
        workdir: container.workdir.clone(),
    };
    let hash = spec_hash(&spec);
    spec.labels.insert("seedling.spec-hash".to_string(), hash);
    spec
}

#[cfg(test)]
mod tests;
