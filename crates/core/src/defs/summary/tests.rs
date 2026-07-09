use std::collections::BTreeMap;

use super::*;

fn deployment_summary_with_image(image: Option<&str>, scale: (u16, u16)) -> DeploymentSummary {
    DeploymentSummary {
        container: ContainerSummary {
            image: image.map(String::from),
            command: None,
            args: None,
            env: BTreeMap::new(),
            volume_mounts: BTreeMap::new(),
            on_exit: "restart",
            memory: None,
            cpus: None,
            extra_caps: Vec::new(),
            writable_rootfs: false,
            pids_limit: None,
            workdir: None,
            healthcheck: None,
            stop_signal: None,
            stop_timeout_secs: None,
        },
        pod: PodSummary {
            service_mounts: Vec::new(),
            http_bindings: Vec::new(),
            tcp_bindings: Vec::new(),
            udp_bindings: Vec::new(),
        },
        scale: ScaleSummary {
            low: scale.0,
            high: scale.1,
        },
        on_update: "rolling",
        on_terminate: "recreate",
        description: None,
    }
}

#[test]
fn diff_fields_returns_empty_for_equal_summaries() {
    let s1 = ResourceSummary::Deployment(deployment_summary_with_image(Some("nginx:1"), (1, 1)));
    let s2 = ResourceSummary::Deployment(deployment_summary_with_image(Some("nginx:1"), (1, 1)));
    assert!(diff_fields(&s1, &s2).is_empty());
}

#[test]
fn diff_fields_reports_only_differing_top_level_fields() {
    let s1 = ResourceSummary::Deployment(deployment_summary_with_image(Some("nginx:1"), (1, 1)));
    let s2 = ResourceSummary::Deployment(deployment_summary_with_image(Some("nginx:2"), (1, 1)));
    let fields = diff_fields(&s1, &s2);
    assert_eq!(fields, vec!["container".to_string()]);
}

#[test]
fn diff_fields_reports_scale_change() {
    let s1 = ResourceSummary::Deployment(deployment_summary_with_image(Some("nginx:1"), (1, 1)));
    let s2 = ResourceSummary::Deployment(deployment_summary_with_image(Some("nginx:1"), (1, 3)));
    let fields = diff_fields(&s1, &s2);
    assert_eq!(fields, vec!["scale".to_string()]);
}

#[test]
fn diff_fields_reports_multiple_changes() {
    let s1 = ResourceSummary::Deployment(deployment_summary_with_image(Some("nginx:1"), (1, 1)));
    let s2 = ResourceSummary::Deployment(deployment_summary_with_image(Some("nginx:2"), (1, 3)));
    let fields = diff_fields(&s1, &s2);
    assert_eq!(fields, vec!["container".to_string(), "scale".to_string()]);
}

#[test]
fn diff_fields_skips_kind_discriminator() {
    let s = ResourceSummary::Deployment(deployment_summary_with_image(Some("nginx:1"), (1, 1)));
    let fields = diff_fields(&s, &s);
    assert!(
        fields.is_empty(),
        "no fields should be reported when summaries are identical"
    );
}

#[test]
fn ingress_summary_captures_http_termination_string() {
    let s = IngressSummary {
        service: "public".into(),
        hostname: "example.com".into(),
        port: 443,
        tls: true,
        dtls: false,
        http_terminate: Some("http2"),
        redirect: None,
        description: None,
    };
    let json = serde_json::to_value(&s).unwrap();
    assert_eq!(json["http_terminate"], "http2");
}

#[test]
fn volume_mount_summary_distinguishes_volume_kinds() {
    let internal = VolumeMountSummary::Volume {
        name: Some("data".into()),
    };
    let external = VolumeMountSummary::ExternalVolume {
        name: "shared".into(),
    };
    let internal_json = serde_json::to_value(&internal).unwrap();
    let external_json = serde_json::to_value(&external).unwrap();
    assert_eq!(internal_json["kind"], "volume");
    assert_eq!(external_json["kind"], "external_volume");
}

#[test]
fn end_to_end_real_app_diff() {
    // Build two Apps from real BSL scripts and verify the summary diff
    // surfaces the actual user-facing change (image bump) without noise.
    use crate::defs::resource::{ResourceId, ResourceKind};

    let limits = crate::ScriptLimits::default();
    let mut current_params = std::collections::BTreeMap::new();
    current_params.insert("ver".to_string(), "1.0".to_string());
    let mut proposed_params = std::collections::BTreeMap::new();
    proposed_params.insert("ver".to_string(), "2.0".to_string());

    let script = r#"
        let v = app.param("ver").value();
        app.deployment("web").image(`ghcr.io/example/web:${v}`).scale(2);
    "#;

    let test_app = seedling_protocol::names::AppName::new("test").unwrap();
    let (cur, cur_err) =
        crate::runtime::apps::evaluate_script(&test_app, script, &current_params, &limits);
    assert!(cur_err.is_none(), "current eval: {cur_err:?}");
    let (prop, prop_err) =
        crate::runtime::apps::evaluate_script(&test_app, script, &proposed_params, &limits);
    assert!(prop_err.is_none(), "proposed eval: {prop_err:?}");

    let id = ResourceId {
        kind: ResourceKind::Deployment,
        name: std::sync::Arc::new("web".to_string()),
    };
    let cur_resource = cur.def.load().resources.get(&id).cloned().unwrap();
    let prop_resource = prop.def.load().resources.get(&id).cloned().unwrap();

    let fields = diff_fields(&cur_resource.summary(), &prop_resource.summary());
    assert_eq!(
        fields,
        vec!["container".to_string()],
        "only the container (with the new image) should differ"
    );
}

// i[verify plan.dry-run]
#[test]
fn container_def_summary_captures_env_on_exit_and_healthcheck() {
    use seedling_protocol::env::EnvVar;

    use crate::defs::container::{
        ContainerDef, HealthcheckDef, HealthcheckKind, HealthcheckOnFailure,
    };
    use crate::defs::enums::OnExit;

    let def = ContainerDef {
        image: Some("docker.io/library/nginx:1.25".to_owned()),
        env: vec![
            EnvVar::new("ZED", "last").unwrap(),
            EnvVar::new("ALPHA", "first").unwrap(),
        ],
        on_exit: Some(OnExit::RestartOnFailure),
        healthcheck: Some(HealthcheckDef {
            kind: HealthcheckKind::Command {
                cmd: vec!["/bin/check".to_owned()],
            },
            interval_secs: 7,
            timeout_secs: 3,
            retries: 2,
            start_period_secs: 15,
            on_failure: HealthcheckOnFailure::Monitor,
        }),
        ..ContainerDef::default()
    };

    let summary = def.summary();
    assert_eq!(summary.on_exit, "restart_on_failure");
    // Env vars land in an ordered map keyed by name.
    let keys: Vec<&str> = summary.env.keys().map(String::as_str).collect();
    assert_eq!(keys, ["ALPHA", "ZED"]);

    let hc = summary.healthcheck.expect("healthcheck summarised");
    assert_eq!(hc.kind, "command");
    assert_eq!(hc.cmd.as_deref(), Some(&["/bin/check".to_owned()][..]));
    assert_eq!(hc.interval_secs, 7);
    assert_eq!(hc.on_failure, "monitor");
}

// i[verify plan.dry-run]
#[test]
fn container_def_summary_defaults_on_exit_to_default() {
    use crate::defs::container::ContainerDef;

    let summary = ContainerDef::default().summary();
    assert_eq!(summary.on_exit, "default");
    assert!(summary.healthcheck.is_none());
}

// i[verify plan.dry-run]
#[test]
fn volume_summary_captures_writes_and_export() {
    use crate::defs::export::ExportOptions;
    use crate::defs::volume::{Volume, VolumeDef};

    let vol = Volume::new(Some(std::sync::Arc::new("data".to_owned())));
    {
        let mut def = vol.def.lock();
        *def = VolumeDef {
            read_only: true,
            tmpfs: false,
            writes: vec![("/etc/motd".to_owned(), "hello".to_owned())],
            exported: Some(ExportOptions {
                description: Some("shared data".to_owned()),
            }),
            description: Some("main volume".to_owned()),
        };
    }

    let summary = vol.summary();
    assert!(summary.readonly);
    assert!(!summary.tmpfs);
    assert_eq!(
        summary.writes.get("/etc/motd").map(String::as_str),
        Some("hello")
    );
    assert!(summary.exported);
    assert_eq!(summary.export_description.as_deref(), Some("shared data"));
    assert_eq!(summary.description.as_deref(), Some("main volume"));
}

// i[verify plan.dry-run]
#[test]
fn diff_fields_reports_volume_writes_change() {
    use crate::defs::volume::Volume;

    let a = Volume::new(Some(std::sync::Arc::new("data".to_owned())));
    let b = Volume::new(Some(std::sync::Arc::new("data".to_owned())));
    b.def
        .lock()
        .writes
        .push(("/etc/motd".to_owned(), "hello".to_owned()));

    let fields = diff_fields(
        &ResourceSummary::Volume(a.summary()),
        &ResourceSummary::Volume(b.summary()),
    );
    assert_eq!(fields, vec!["writes".to_owned()]);
}

// i[verify plan.dry-run]
#[test]
fn service_summary_flags_http_and_export() {
    use crate::defs::export::ExportOptions;
    use crate::defs::service::{Service, ServiceDef};

    let service = Service {
        name: std::sync::Arc::new("db".to_owned()),
        def: std::sync::Arc::new(parking_lot::Mutex::new(ServiceDef {
            http: None,
            exported: Some(ExportOptions {
                description: Some("database".to_owned()),
            }),
            description: None,
        })),
        app_def: None,
        frozen: false,
    };

    let summary = service.summary();
    assert!(!summary.http);
    assert!(summary.exported);
    assert_eq!(summary.export_description.as_deref(), Some("database"));
    assert!(summary.description.is_none());
}
