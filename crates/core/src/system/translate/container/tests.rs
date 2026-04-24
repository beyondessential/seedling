use std::collections::BTreeMap;

use seedling_protocol::env::EnvVar;

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
        env: vec![EnvVar::new("PORT", "8080").unwrap()],
        mounts: vec![],
        network: "seedling-abc123".to_string(),
        labels: BTreeMap::new(),
        health: None,
        hosts: vec![],
        dns_servers: vec![],
        memory: None,
        cpus: None,
        extra_caps: vec![],
        writable_rootfs: false,
        pids_limit: 256,
        workdir: None,
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
        labels: BTreeMap::new(),
        health: None,
        hosts: vec![],
        dns_servers: vec![],
        memory: None,
        cpus: None,
        extra_caps: vec![],
        writable_rootfs: false,
        pids_limit: 256,
        workdir: None,
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
        labels: BTreeMap::new(),
        health: None,
        hosts: vec![("localmount".to_string(), IpAddr::V6(ip))],
        dns_servers: vec![],
        memory: None,
        cpus: None,
        extra_caps: vec![],
        writable_rootfs: false,
        pids_limit: 256,
        workdir: None,
    };

    let args = podman_args(&spec);
    let host_pos = args.iter().position(|a| a == "--add-host").unwrap();
    assert_eq!(args[host_pos + 1], "localmount:fd5e:ed12:3456:500::2");
}

// r[verify actuate.container.hardening]
// r[verify actuate.container.journal-metadata]
#[test]
fn podman_args_hardening_defaults() {
    let spec = ContainerSpec {
        name: "n".to_string(),
        image: "img".to_string(),
        command: vec![],
        entrypoint: vec![],
        env: vec![],
        mounts: vec![],
        network: "net".to_string(),
        labels: BTreeMap::new(),
        health: None,
        hosts: vec![],
        dns_servers: vec![],
        memory: None,
        cpus: None,
        extra_caps: vec![],
        writable_rootfs: false,
        pids_limit: 256,
        workdir: None,
    };

    let args = podman_args(&spec);

    assert!(args.contains(&"--cap-drop=ALL".to_string()));
    assert!(!args.iter().any(|a| a.starts_with("--cap-add")));
    assert!(args.contains(&"--security-opt".to_string()));
    let secopt_pos = args.iter().position(|a| a == "--security-opt").unwrap();
    assert_eq!(args[secopt_pos + 1], "no-new-privileges");
    assert!(args.contains(&"--read-only".to_string()));
    let tmpfs_pos = args.iter().position(|a| a == "--tmpfs").unwrap();
    assert_eq!(args[tmpfs_pos + 1], "/tmp");
    let pids_pos = args.iter().position(|a| a == "--pids-limit").unwrap();
    assert_eq!(args[pids_pos + 1], "256");
    let ulimit_pos = args.iter().position(|a| a == "--ulimit").unwrap();
    assert_eq!(args[ulimit_pos + 1], "nofile=65536:65536");
    assert!(!args.iter().any(|a| a.starts_with("--memory")));
    assert!(!args.iter().any(|a| a.starts_with("--cpus")));
}

// r[verify actuate.container.hardening]
#[test]
fn podman_args_hardening_overrides() {
    let spec = ContainerSpec {
        name: "n".to_string(),
        image: "img".to_string(),
        command: vec![],
        entrypoint: vec![],
        env: vec![],
        mounts: vec![],
        network: "net".to_string(),
        labels: BTreeMap::new(),
        health: None,
        hosts: vec![],
        dns_servers: vec![],
        memory: Some("512m".to_string()),
        cpus: Some(1.5),
        extra_caps: vec!["NET_RAW".to_string(), "NET_BIND_SERVICE".to_string()],
        writable_rootfs: true,
        pids_limit: 1024,
        workdir: None,
    };

    let args = podman_args(&spec);

    assert!(args.contains(&"--cap-drop=ALL".to_string()));
    assert!(args.contains(&"--cap-add=NET_RAW".to_string()));
    assert!(args.contains(&"--cap-add=NET_BIND_SERVICE".to_string()));
    assert!(!args.contains(&"--read-only".to_string()));
    assert!(!args.iter().any(|a| a == "--tmpfs"));
    let pids_pos = args.iter().position(|a| a == "--pids-limit").unwrap();
    assert_eq!(args[pids_pos + 1], "1024");
    assert!(args.contains(&"--memory=512m".to_string()));
    assert!(args.contains(&"--cpus=1.5".to_string()));
    assert!(!args.iter().any(|a| a.starts_with("--workdir")));
}

// r[verify infra.pod.dns]
#[test]
fn podman_args_dns_servers_produce_dns_flags() {
    use std::net::Ipv6Addr;
    use std::str::FromStr;

    let spec = ContainerSpec {
        name: "n".to_string(),
        image: "img".to_string(),
        command: vec![],
        entrypoint: vec![],
        env: vec![],
        mounts: vec![],
        network: "net".to_string(),
        labels: BTreeMap::new(),
        health: None,
        hosts: vec![],
        dns_servers: vec![
            Ipv6Addr::from_str("fd5e:ed12:3456:0500::1").unwrap(),
            Ipv6Addr::from_str("fd5e:ed12:3456:0500::2").unwrap(),
        ],
        memory: None,
        cpus: None,
        extra_caps: vec![],
        writable_rootfs: false,
        pids_limit: 256,
        workdir: None,
    };

    let args = podman_args(&spec);
    let dns_positions: Vec<_> = args
        .iter()
        .enumerate()
        .filter_map(|(i, a)| if a == "--dns" { Some(i) } else { None })
        .collect();
    assert_eq!(dns_positions.len(), 2);
    assert_eq!(args[dns_positions[0] + 1], "fd5e:ed12:3456:500::1");
    assert_eq!(args[dns_positions[1] + 1], "fd5e:ed12:3456:500::2");
}

// r[verify infra.pod.dns]
#[test]
fn podman_args_no_dns_flags_when_unset() {
    let spec = ContainerSpec {
        name: "n".to_string(),
        image: "img".to_string(),
        command: vec![],
        entrypoint: vec![],
        env: vec![],
        mounts: vec![],
        network: "net".to_string(),
        labels: BTreeMap::new(),
        health: None,
        hosts: vec![],
        dns_servers: vec![],
        memory: None,
        cpus: None,
        extra_caps: vec![],
        writable_rootfs: false,
        pids_limit: 256,
        workdir: None,
    };
    let args = podman_args(&spec);
    assert!(!args.iter().any(|a| a == "--dns"));
}

// l[verify container.workdir]
#[test]
fn podman_args_workdir() {
    let spec = ContainerSpec {
        name: "n".to_string(),
        image: "img".to_string(),
        command: vec![],
        entrypoint: vec![],
        env: vec![],
        mounts: vec![],
        network: "net".to_string(),
        labels: BTreeMap::new(),
        health: None,
        hosts: vec![],
        dns_servers: vec![],
        memory: None,
        cpus: None,
        extra_caps: vec![],
        writable_rootfs: false,
        pids_limit: 256,
        workdir: Some("/app".to_string()),
    };
    let args = podman_args(&spec);
    assert!(args.contains(&"--workdir=/app".to_string()));
}

fn bare_spec() -> ContainerSpec {
    ContainerSpec {
        name: "myapp-web".to_string(),
        image: "docker.io/library/nginx:latest".to_string(),
        command: vec![],
        entrypoint: vec![],
        env: vec![],
        mounts: vec![],
        network: "seedling-net".to_string(),
        labels: BTreeMap::new(),
        health: None,
        hosts: vec![],
        dns_servers: vec![],
        memory: None,
        cpus: None,
        extra_caps: vec![],
        writable_rootfs: false,
        pids_limit: 256,
        workdir: None,
    }
}

// r[verify update.spec-hash]
#[test]
fn spec_hash_is_stable_for_same_spec() {
    let spec = bare_spec();
    let h1 = spec_hash(&spec);
    let h2 = spec_hash(&spec);
    assert_eq!(h1, h2);
    assert_eq!(h1.len(), 64, "spec hash is SHA-256 hex");
}

// r[verify update.spec-hash]
#[test]
fn spec_hash_changes_when_image_changes() {
    let mut a = bare_spec();
    let mut b = bare_spec();
    a.image = "docker.io/library/nginx:1.25".to_string();
    b.image = "docker.io/library/nginx:1.26".to_string();
    assert_ne!(spec_hash(&a), spec_hash(&b));
}

// r[verify update.spec-hash]
#[test]
fn spec_hash_changes_when_env_changes() {
    use seedling_protocol::env::EnvVar;
    let mut a = bare_spec();
    let mut b = bare_spec();
    a.env = vec![EnvVar::new("KEY", "old").unwrap()];
    b.env = vec![EnvVar::new("KEY", "new").unwrap()];
    assert_ne!(spec_hash(&a), spec_hash(&b));
}

// r[verify update.spec-hash]
#[test]
fn spec_hash_changes_when_memory_or_cpus_change() {
    let mut a = bare_spec();
    let mut b = bare_spec();
    a.memory = Some("256m".to_string());
    b.memory = Some("512m".to_string());
    assert_ne!(spec_hash(&a), spec_hash(&b));

    let mut c = bare_spec();
    let mut d = bare_spec();
    c.cpus = Some(1.0);
    d.cpus = Some(2.0);
    assert_ne!(spec_hash(&c), spec_hash(&d));
}

// r[verify update.spec-hash]
#[test]
fn spec_hash_ignores_self_label() {
    // The stored `seedling.spec-hash` label is excluded from the hash input
    // so that re-hashing a running container's observed spec (which includes
    // its own hash label) does not produce a different result.
    let mut a = bare_spec();
    let mut b = bare_spec();
    a.labels
        .insert("seedling.spec-hash".to_string(), "old-hash".to_string());
    b.labels.insert(
        "seedling.spec-hash".to_string(),
        "different-hash".to_string(),
    );
    assert_eq!(spec_hash(&a), spec_hash(&b));
}

// r[verify update.spec-hash]
#[test]
fn spec_hash_considers_other_labels() {
    let mut a = bare_spec();
    let mut b = bare_spec();
    a.labels
        .insert("seedling.app".to_string(), "alpha".to_string());
    b.labels
        .insert("seedling.app".to_string(), "beta".to_string());
    assert_ne!(spec_hash(&a), spec_hash(&b));
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
        app: seedling_protocol::names::AppName::new("myapp").unwrap(),
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
        &[],
        None,
        &std::collections::HashMap::new(),
        0,
    );

    // The volume mount source must be the canonical Volume-resource display
    // name so the bind-mount resolves to the same on-disk path the Volume
    // actuator creates.
    let vol_mount = spec
        .mounts
        .iter()
        .find(|m| m.target == "/mnt/data")
        .expect("mount at /mnt/data present");
    assert!(
        matches!(&vol_mount.source, MountSource::Volume(n) if n == "myapp-volume-data"),
        "expected myapp-volume-data, got {:?}",
        vol_mount.source,
    );
}

// r[verify healthcheck.on-failure]
#[test]
fn podman_args_emit_health_check_flags_when_declared() {
    use std::time::Duration;

    use crate::system::types::{HealthCheckOnFailure, HealthCheckSpec};

    let spec = ContainerSpec {
        name: "n".to_string(),
        image: "img".to_string(),
        command: vec![],
        entrypoint: vec![],
        env: vec![],
        mounts: vec![],
        network: "net".to_string(),
        labels: BTreeMap::new(),
        health: Some(HealthCheckSpec {
            command: vec!["/bin/check".to_string()],
            interval: Duration::from_secs(7),
            timeout: Duration::from_secs(3),
            retries: 2,
            start_period: Duration::from_secs(15),
            on_failure: HealthCheckOnFailure::Restart,
        }),
        hosts: vec![],
        dns_servers: vec![],
        memory: None,
        cpus: None,
        extra_caps: vec![],
        writable_rootfs: false,
        pids_limit: 256,
        workdir: None,
    };

    let args = podman_args(&spec);
    let find = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .map(|p| args[p + 1].clone())
            .unwrap_or_else(|| panic!("flag not found: {flag}"))
    };

    assert_eq!(find("--health-cmd"), "[\"/bin/check\"]");
    assert_eq!(find("--health-interval"), "7s");
    assert_eq!(find("--health-timeout"), "3s");
    assert_eq!(find("--health-retries"), "2");
    assert_eq!(find("--health-start-period"), "15s");
    assert_eq!(find("--health-on-failure"), "restart");
}

#[test]
fn podman_args_omit_health_check_flags_when_absent() {
    let spec = ContainerSpec {
        name: "n".to_string(),
        image: "img".to_string(),
        command: vec![],
        entrypoint: vec![],
        env: vec![],
        mounts: vec![],
        network: "net".to_string(),
        labels: BTreeMap::new(),
        health: None,
        hosts: vec![],
        dns_servers: vec![],
        memory: None,
        cpus: None,
        extra_caps: vec![],
        writable_rootfs: false,
        pids_limit: 256,
        workdir: None,
    };

    let args = podman_args(&spec);
    assert!(!args.iter().any(|a| a.starts_with("--health-")));
}
