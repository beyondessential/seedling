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
        stop_signal: None,
        stop_timeout_secs: None,
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
        stop_signal: None,
        stop_timeout_secs: None,
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
        stop_signal: None,
        stop_timeout_secs: None,
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
        stop_signal: None,
        stop_timeout_secs: None,
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
        stop_signal: None,
        stop_timeout_secs: None,
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
        stop_signal: None,
        stop_timeout_secs: None,
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
        stop_signal: None,
        stop_timeout_secs: None,
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
        stop_signal: None,
        stop_timeout_secs: None,
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
        stop_signal: None,
        stop_timeout_secs: None,
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

// r[verify autonomous.healthcheck-replace]
#[test]
fn podman_args_emit_health_check_flags_when_declared() {
    use std::time::Duration;

    use crate::system::types::HealthCheckSpec;

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
        }),
        hosts: vec![],
        dns_servers: vec![],
        memory: None,
        cpus: None,
        extra_caps: vec![],
        writable_rootfs: false,
        pids_limit: 256,
        workdir: None,
        stop_signal: None,
        stop_timeout_secs: None,
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
    // Seedling always tells podman not to act; replace logic is reconciler-driven.
    assert_eq!(find("--health-on-failure"), "none");
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
        stop_signal: None,
        stop_timeout_secs: None,
    };

    let args = podman_args(&spec);
    assert!(!args.iter().any(|a| a.starts_with("--health-")));
}

// -----------------------------------------------------------------------
// podman_args — mounts and entrypoint
// -----------------------------------------------------------------------

#[test]
fn podman_args_tmpfs_and_bind_mounts() {
    let mut spec = bare_spec();
    spec.mounts = vec![
        Mount {
            source: MountSource::Tmpfs,
            target: "/scratch".to_string(),
            read_only: false,
        },
        Mount {
            source: MountSource::Bind("/host/data".into()),
            target: "/data".to_string(),
            read_only: true,
        },
        Mount {
            source: MountSource::Bind("/host/rw".into()),
            target: "/rw".to_string(),
            read_only: false,
        },
    ];

    let args = podman_args(&spec);
    let mount_pos = args.iter().position(|a| a == "--mount").unwrap();
    assert_eq!(args[mount_pos + 1], "type=tmpfs,destination=/scratch");

    let volume_args: Vec<&str> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--volume")
        .map(|(i, _)| args[i + 1].as_str())
        .collect();
    assert_eq!(volume_args, ["/host/data:/data:ro", "/host/rw:/rw"]);
}

#[test]
fn podman_args_entrypoint_override_clears_image_entrypoint() {
    let mut spec = bare_spec();
    spec.entrypoint = vec!["/bin/custom".to_string()];
    spec.command = vec!["--flag".to_string()];

    let args = podman_args(&spec);
    let ep_pos = args.iter().position(|a| a == "--entrypoint").unwrap();
    assert_eq!(args[ep_pos + 1], "[]");

    // Entrypoint args come right after the image, followed by cmd args.
    let image_pos = args.iter().position(|a| a == &spec.image).unwrap();
    assert_eq!(&args[image_pos + 1..], &["/bin/custom", "--flag"]);
}

#[test]
fn podman_args_no_entrypoint_flag_without_override() {
    let mut spec = bare_spec();
    spec.command = vec!["serve".to_string()];
    let args = podman_args(&spec);
    assert!(!args.iter().any(|a| a == "--entrypoint"));
}

// -----------------------------------------------------------------------
// spec_from_pod — via deployment_spec
// -----------------------------------------------------------------------

fn make_instance(app: &str, name: &str) -> ResourceInstance {
    use crate::defs::resource::ResourceKind;
    use crate::runtime::identity::{InstanceId, InstanceVariant};

    ResourceInstance {
        id: InstanceId::generate(),
        app: seedling_protocol::names::AppName::new(app).unwrap(),
        kind: ResourceKind::Deployment,
        name: Some(name.to_string()),
        variant: InstanceVariant::Singleton,
        display_name: format!("{app}-{name}"),
    }
}

fn network() -> (String, Ipv6Net) {
    (
        "seedling-myapp-web".to_string(),
        "fd5e:ed12:3456:0100::/64".parse().unwrap(),
    )
}

fn spec_for_deployment(
    def: &crate::defs::deployment::DeploymentDef,
    mounts: &[(u16, Ipv6Addr, u16)],
    volumes_dir: Option<&Path>,
    external_volumes: &HashMap<ExternalVolumeName, ResolvedExternalMount>,
    restart_gen: u64,
) -> ContainerSpec {
    deployment_spec(
        def,
        &make_instance("myapp", "web"),
        &BTreeMap::new(),
        &network(),
        mounts,
        &[],
        volumes_dir,
        external_volumes,
        restart_gen,
    )
}

// l[verify volume.external.dynamic]
#[test]
fn external_volume_resolution_prefers_operation_binding_then_static_mapping() {
    use std::sync::Arc;

    use crate::defs::container::VolumeMount;
    use crate::defs::deployment::DeploymentDef;
    use crate::defs::volume::{ExternalVolume, ExternalVolumeDef, OperationVolumeBinding};

    fn ext_vol(name: &str, binding: Option<OperationVolumeBinding>) -> ExternalVolume {
        ExternalVolume {
            name: Arc::new(name.to_string()),
            operation_binding: binding,
            def: Arc::new(parking_lot::Mutex::new(ExternalVolumeDef::default())),
        }
    }

    let def = DeploymentDef::default();
    {
        let pod = def.pod.lock();
        let mut container = pod.container.lock();
        container.image = Some("img".to_string());
        container.volume_mounts.insert(
            "/op".into(),
            VolumeMount::ExternalVolume(ext_vol(
                "bound",
                Some(OperationVolumeBinding {
                    host_path: "/run/op-bind".into(),
                    read_only: true,
                }),
            )),
        );
        container.volume_mounts.insert(
            "/static".into(),
            VolumeMount::ExternalVolume(ext_vol("mapped", None)),
        );
        container.volume_mounts.insert(
            "/unmapped".into(),
            VolumeMount::ExternalVolume(ext_vol("ghost", None)),
        );
    }

    let mut external_volumes = HashMap::new();
    external_volumes.insert(
        ExternalVolumeName::new_unchecked("mapped"),
        ResolvedExternalMount {
            source: MountSource::Bind("/srv/static".into()),
            read_only: true,
        },
    );

    let spec = spec_for_deployment(&def, &[], None, &external_volumes, 0);
    let mount = |target: &str| {
        spec.mounts
            .iter()
            .find(|m| m.target == target)
            .unwrap_or_else(|| panic!("mount at {target}"))
    };

    // Operation-scoped binding wins even though no static mapping exists.
    let op = mount("/op");
    assert!(matches!(&op.source, MountSource::Bind(p) if p == Path::new("/run/op-bind")));
    assert!(op.read_only);

    // Static mapping applies when no operation binding is present.
    let stat = mount("/static");
    assert!(matches!(&stat.source, MountSource::Bind(p) if p == Path::new("/srv/static")));
    assert!(stat.read_only);

    // Unmapped external volumes fall back to a named podman volume.
    let ghost = mount("/unmapped");
    assert!(matches!(&ghost.source, MountSource::Volume(n) if n == "ghost"));
    assert!(!ghost.read_only);
}

// r[verify actuate.volume.tmpfs]
// r[verify actuate.volume.storage]
#[test]
fn volume_mount_sources_resolve_by_volume_kind() {
    use std::sync::Arc;

    use crate::defs::container::VolumeMount;
    use crate::defs::deployment::DeploymentDef;
    use crate::defs::volume::Volume;

    let def = DeploymentDef::default();
    {
        let pod = def.pod.lock();
        let mut container = pod.container.lock();
        container.image = Some("img".to_string());

        let tmpfs_vol = Volume::new(Some(Arc::new("cache".to_string())));
        tmpfs_vol.def.lock().tmpfs = true;
        container
            .volume_mounts
            .insert("/cache".into(), VolumeMount::Volume(tmpfs_vol));

        container.volume_mounts.insert(
            "/data".into(),
            VolumeMount::Volume(Volume::new(Some(Arc::new("data".to_string())))),
        );

        container.volume_mounts.insert(
            "/anon".into(),
            VolumeMount::Volume(Volume::new_anonymous("anon-1234".to_string())),
        );
    }

    let volumes_dir = Path::new("/var/lib/seedling/volumes");
    let spec = spec_for_deployment(&def, &[], Some(volumes_dir), &HashMap::new(), 0);
    let source = |target: &str| {
        &spec
            .mounts
            .iter()
            .find(|m| m.target == target)
            .unwrap_or_else(|| panic!("mount at {target}"))
            .source
    };

    // Tmpfs volumes bind from the tmpfs-backed host dir, even with a volumes_dir set.
    let expected_tmpfs = std::path::PathBuf::from(crate::system::actuator::TMPFS_VOLUMES_DIR)
        .join("myapp-volume-cache");
    assert!(matches!(source("/cache"), MountSource::Bind(p) if *p == expected_tmpfs));

    // Named volumes bind from the volumes dir using the canonical display name.
    let expected_named = volumes_dir.join("myapp-volume-data");
    assert!(matches!(source("/data"), MountSource::Bind(p) if *p == expected_named));

    // Anonymous volumes stay podman-managed under their anon id.
    assert!(matches!(source("/anon"), MountSource::Volume(n) if n == "anon-1234"));
}

// i[verify deployment.restart]
#[test]
fn restart_gen_label_is_set_only_when_positive() {
    use crate::defs::deployment::DeploymentDef;

    let def = DeploymentDef::default();
    def.pod.lock().container.lock().image = Some("img".to_string());

    let spec = spec_for_deployment(&def, &[], None, &HashMap::new(), 3);
    assert_eq!(
        spec.labels.get("seedling.restart-gen").map(String::as_str),
        Some("3")
    );

    let spec = spec_for_deployment(&def, &[], None, &HashMap::new(), 0);
    assert!(!spec.labels.contains_key("seedling.restart-gen"));
}

// l[verify deployment.healthcheck]
#[test]
fn pod_healthcheck_and_service_mounts_translate_into_spec() {
    use crate::defs::container::{HealthcheckDef, HealthcheckKind, HealthcheckOnFailure};
    use crate::defs::deployment::DeploymentDef;
    use crate::system::translate::proxy::node_mount_addr;

    let def = DeploymentDef::default();
    {
        let pod = def.pod.lock();
        let mut container = pod.container.lock();
        container.image = Some("img".to_string());
        container.healthcheck = Some(HealthcheckDef {
            kind: HealthcheckKind::Command {
                cmd: vec!["/bin/check".to_string()],
            },
            interval_secs: 7,
            timeout_secs: 3,
            retries: 2,
            start_period_secs: 15,
            on_failure: HealthcheckOnFailure::Replace,
        });
    }

    let mount_addr: Ipv6Addr = "fd5e:ed12:3456:100::2".parse().unwrap();
    let spec = spec_for_deployment(&def, &[(80, mount_addr, 8080)], None, &HashMap::new(), 0);

    let health = spec.health.as_ref().expect("healthcheck translated");
    assert_eq!(health.command, ["/bin/check"]);
    assert_eq!(health.interval, Duration::from_secs(7));
    assert_eq!(health.timeout, Duration::from_secs(3));
    assert_eq!(health.retries, 2);
    assert_eq!(health.start_period, Duration::from_secs(15));

    // A pod with service mounts gets the `localmount` host entry pointing at
    // the node-side mount endpoint.
    let expected = node_mount_addr(&network().1);
    assert_eq!(
        spec.hosts,
        vec![("localmount".to_string(), IpAddr::V6(expected))]
    );
}
