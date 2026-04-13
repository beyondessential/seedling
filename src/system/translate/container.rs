use std::{
    collections::{BTreeMap, HashMap},
    net::{IpAddr, Ipv6Addr},
};

use ipnet::Ipv6Net;

use crate::{
    defs::{container::VolumeMount, deployment::DeploymentDef, job::JobDef, pod::PodDef},
    runtime::identity::ResourceInstance,
    system::{
        translate::proxy::node_mount_addr,
        types::{ContainerSpec, Mount, MountSource},
    },
};

/// Builds a `ContainerSpec` for one instance of a `Deployment`.
pub fn deployment_spec(
    def: &DeploymentDef,
    instance: &ResourceInstance,
    _params: &BTreeMap<String, String>,
    network: &(String, Ipv6Net),
    mounts: &[(u16, Ipv6Addr, u16)],
) -> ContainerSpec {
    let pod = def.pod.lock();
    spec_from_pod(&pod, instance, network, mounts)
}

/// Builds a `ContainerSpec` for a `Job` instance.
pub fn job_spec(
    def: &JobDef,
    instance: &ResourceInstance,
    _params: &BTreeMap<String, String>,
    network: &(String, Ipv6Net),
    mounts: &[(u16, Ipv6Addr, u16)],
) -> ContainerSpec {
    let pod = def.pod.lock();
    spec_from_pod(&pod, instance, network, mounts)
}

/// Produces the `podman run [...]` argv from a `ContainerSpec`.
///
/// This is the `ExecStart` value for the transient systemd unit. The returned
/// vec begins with `["podman", "run", "--rm", ...]` and ends with the image
/// reference followed by any command arguments.
pub fn podman_args(spec: &ContainerSpec) -> Vec<String> {
    let mut args = vec!["podman".to_string(), "run".to_string(), "--rm".to_string()];

    args.push("--name".to_string());
    args.push(spec.name.clone());

    args.push("--network".to_string());
    args.push(spec.network.clone());

    for (k, v) in &spec.env {
        args.push("--env".to_string());
        args.push(format!("{k}={v}"));
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

/// Derive a deterministic podman volume name for an anonymous BSL volume
/// mounted at `mount_path` on `instance`.
///
/// The name is stable across restarts (same instance + same path → same
/// name) but unique per (instance, path) pair.
pub fn anon_vol_name(instance: &ResourceInstance, mount_path: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(mount_path.as_bytes());
    format!(
        "{}-anon-{:02x}{:02x}{:02x}{:02x}",
        instance.display_name, hash[0], hash[1], hash[2], hash[3]
    )
}

// ---------------------------------------------------------------------------
// Shared pod → spec logic
// ---------------------------------------------------------------------------

fn spec_from_pod(
    pod: &PodDef,
    instance: &ResourceInstance,
    network: &(String, Ipv6Net),
    mounts: &[(u16, Ipv6Addr, u16)],
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
            let source = match vm {
                VolumeMount::Volume(v) => {
                    let name = match &v.name {
                        Some(n) => format!("{}-{}", instance.app, n.as_str()),
                        None => match &v.anon_id {
                            Some(id) => id.clone(),
                            None => anon_vol_name(instance, &path.to_string_lossy()),
                        },
                    };
                    MountSource::Volume(name)
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

    let mut labels = HashMap::new();
    labels.insert("seedling.app".to_string(), instance.app.clone());
    labels.insert("seedling.instance".to_string(), instance.id.to_hex());
    labels.insert("seedling.kind".to_string(), format!("{:?}", instance.kind));
    labels.insert(
        "seedling.display-name".to_string(),
        instance.display_name.clone(),
    );

    // Inject the mount endpoint host entry so that containers can reach
    // mounted services via `localmount:<port>`. The ::2 address lives on
    // the host side of the pod bridge.
    let hosts = if mounts.is_empty() {
        vec![]
    } else {
        let mount_endpoint = node_mount_addr(&network.1);
        vec![("localmount".to_string(), IpAddr::V6(mount_endpoint))]
    };

    let mut spec = ContainerSpec {
        name: instance.display_name.clone(),
        image,
        command,
        entrypoint,
        env,
        mounts: sys_mounts,
        network: network.0.clone(),
        labels,
        health: None,
        hosts,
    };
    let hash = spec_hash(&spec);
    spec.labels.insert("seedling.spec-hash".to_string(), hash);
    spec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn podman_args_basic_shape() {
        let spec = ContainerSpec {
            name: "myapp-web".to_string(),
            image: "docker.io/library/nginx:latest".to_string(),
            command: vec![
                "nginx".to_string(),
                "-g".to_string(),
                "daemon off;".to_string(),
            ],
            entrypoint: vec![],
            env: vec![("PORT".to_string(), "8080".to_string())],
            mounts: vec![],
            network: "seedling-abc123".to_string(),
            labels: HashMap::new(),
            health: None,
            hosts: vec![],
        };

        let args = podman_args(&spec);

        assert_eq!(&args[..3], &["podman", "run", "--rm"]);
        assert!(args.contains(&"--name".to_string()));
        assert!(args.contains(&"myapp-web".to_string()));
        assert!(args.contains(&"--network".to_string()));
        assert!(args.contains(&"seedling-abc123".to_string()));
        assert!(args.contains(&"--env".to_string()));
        assert!(args.contains(&"PORT=8080".to_string()));
        assert!(args.contains(&"docker.io/library/nginx:latest".to_string()));

        // command args come after the image
        let image_pos = args
            .iter()
            .position(|a| a == "docker.io/library/nginx:latest")
            .unwrap();
        assert_eq!(args[image_pos + 1], "nginx");
    }

    #[test]
    fn podman_args_volume_mount() {
        let spec = ContainerSpec {
            name: "n".to_string(),
            image: "img".to_string(),
            command: vec![],
            entrypoint: vec![],
            env: vec![],
            mounts: vec![Mount {
                source: MountSource::Volume("my-vol".to_string()),
                target: "/data".to_string(),
                read_only: true,
            }],
            network: "net".to_string(),
            labels: HashMap::new(),
            health: None,
            hosts: vec![],
        };

        let args = podman_args(&spec);
        let vol_pos = args.iter().position(|a| a == "--volume").unwrap();
        assert_eq!(args[vol_pos + 1], "my-vol:/data:ro");
    }

    #[test]
    fn podman_args_add_host_ipv6() {
        use std::str::FromStr;
        let ip = Ipv6Addr::from_str("fd5e:ed12:3456:0500::2").unwrap();
        let spec = ContainerSpec {
            name: "n".to_string(),
            image: "img".to_string(),
            command: vec![],
            entrypoint: vec![],
            env: vec![],
            mounts: vec![],
            network: "net".to_string(),
            labels: HashMap::new(),
            health: None,
            hosts: vec![("localmount".to_string(), IpAddr::V6(ip))],
        };

        let args = podman_args(&spec);
        let host_pos = args.iter().position(|a| a == "--add-host").unwrap();
        assert_eq!(args[host_pos + 1], "localmount:fd5e:ed12:3456:500::2");
    }

    #[test]
    fn node_mount_endpoint_is_fffe_one() {
        let prefix: Ipv6Net = "fd5e:ed12:3456:0500::/64".parse().unwrap();
        let endpoint = node_mount_addr(&prefix);
        let octets = endpoint.octets();
        // First 6 bytes: node prefix bytes from fd5e:ed12:3456
        assert_eq!(&octets[..6], &[0xfd, 0x5e, 0xed, 0x12, 0x34, 0x56]);
        // Bytes 6-7: fffe discriminant
        assert_eq!(octets[6], 0xff);
        assert_eq!(octets[7], 0xfe);
        // Bytes 8-14: zeros
        assert_eq!(&octets[8..15], &[0u8; 7]);
        // Byte 15: 1
        assert_eq!(octets[15], 0x01);
    }

    #[test]
    fn volume_mount_uses_app_prefixed_display_name() {
        use std::sync::Arc;

        use crate::defs::resource::ResourceKind;
        use crate::defs::{container::VolumeMount, deployment::DeploymentDef, volume::Volume};
        use crate::runtime::identity::{InstanceId, InstanceVariant, ResourceInstance};

        // Build a DeploymentDef whose container mounts a named volume "data".
        let dep_def = DeploymentDef::default();
        {
            let pod = dep_def.pod.lock();
            let mut container = pod.container.lock();
            container.image = Some("img".to_string());
            container.volume_mounts.insert(
                std::path::PathBuf::from("/mnt/data"),
                VolumeMount::Volume(Volume::new(Some(Arc::new("data".to_string())))),
            );
        }

        let instance = ResourceInstance {
            id: InstanceId::generate(),
            app: "myapp".to_string(),
            kind: ResourceKind::Deployment,
            name: Some("web".to_string()),
            variant: InstanceVariant::Singleton,
            display_name: "myapp-web".to_string(),
        };

        let prefix: ipnet::Ipv6Net = "fd5e:ed12:3456:0100::/64".parse().unwrap();
        let network = ("seedling-myapp-web".to_string(), prefix);
        let spec = deployment_spec(
            &dep_def,
            &instance,
            &std::collections::BTreeMap::new(),
            &network,
            &[],
        );

        // The volume mount source must be the app-prefixed display name, not the raw BSL name.
        let vol_mount = spec
            .mounts
            .iter()
            .find(|m| m.target == "/mnt/data")
            .expect("mount at /mnt/data present");
        assert!(
            matches!(&vol_mount.source, MountSource::Volume(n) if n == "myapp-data"),
            "expected myapp-data, got {:?}",
            vol_mount.source,
        );
    }
}
