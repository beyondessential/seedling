use std::{
    collections::{BTreeMap, HashMap},
    net::{IpAddr, Ipv6Addr},
};

use ipnet::Ipv6Net;

use crate::{
    defs::{container::VolumeMount, deployment::DeploymentDef, job::JobDef, pod::PodDef},
    runtime::identity::ResourceInstance,
    system::types::{ContainerSpec, Mount, MountSource},
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
        args.push("--entrypoint".to_string());
        // Podman parses a JSON array for multi-component entrypoints.
        args.push(
            serde_json::to_string(&spec.entrypoint).unwrap_or_else(|_| spec.entrypoint[0].clone()),
        );
    }

    args.push(spec.image.clone());
    args.extend(spec.command.iter().cloned());

    args
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

    // BSL `.command()` sets the executable/entrypoint override.
    // BSL `.arg()` appends positional arguments passed after the image.
    // Both combine into ContainerSpec.command (argv passed after the image name
    // in the podman invocation). ContainerSpec.entrypoint is left empty unless
    // a separate override is required.
    let mut command = Vec::new();
    if let Some(cmd) = &container.command {
        command.extend(cmd.iter().cloned());
    }
    if let Some(extra) = &container.args {
        command.extend(extra.iter().cloned());
    }

    let env = container.env.clone();

    let sys_mounts = container
        .volume_mounts
        .iter()
        .map(|(path, vm)| {
            let source = match vm {
                VolumeMount::Volume(v) => {
                    MountSource::Volume(v.name.as_ref().map_or("", |n| n.as_str()).to_string())
                }
                VolumeMount::ExternalVolume(ev) => MountSource::Volume(ev.name.to_string()),
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
        let mount_endpoint = pod_mount_endpoint(&network.1);
        vec![("localmount".to_string(), IpAddr::V6(mount_endpoint))]
    };

    ContainerSpec {
        name: instance.display_name.clone(),
        image,
        command,
        entrypoint: vec![],
        env,
        mounts: sys_mounts,
        network: network.0.clone(),
        labels,
        health: None,
        hosts,
    }
}

/// Returns the `::2` address within a pod /64 prefix — the mount endpoint
/// on the host bridge used for service mount DNAT6.
fn pod_mount_endpoint(pod_prefix: &Ipv6Net) -> Ipv6Addr {
    let mut bytes = pod_prefix.network().octets();
    bytes[8..].fill(0);
    bytes[15] = 2;
    Ipv6Addr::from(bytes)
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
    fn pod_mount_endpoint_is_two() {
        let prefix: Ipv6Net = "fd5e:ed12:3456:0500::/64".parse().unwrap();
        let endpoint = pod_mount_endpoint(&prefix);
        let octets = endpoint.octets();
        assert_eq!(&octets[..8], &prefix.network().octets()[..8]);
        assert_eq!(&octets[8..15], &[0u8; 7]);
        assert_eq!(octets[15], 2);
    }
}
