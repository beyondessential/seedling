use std::collections::BTreeMap;

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

// i[verify container.workdir]
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
        &[],
        None,
        &std::collections::HashMap::new(),
        0,
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
