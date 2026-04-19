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

    let (cur, cur_err) = crate::runtime::apps::evaluate_script("test", script, &current_params, &limits);
    assert!(cur_err.is_none(), "current eval: {cur_err:?}");
    let (prop, prop_err) = crate::runtime::apps::evaluate_script("test", script, &proposed_params, &limits);
    assert!(prop_err.is_none(), "proposed eval: {prop_err:?}");

    let id = ResourceId {
        kind: ResourceKind::Deployment,
        name: std::sync::Arc::new("web".to_string()),
    };
    let cur_resource = cur.def.lock().resources.get(&id).cloned().unwrap();
    let prop_resource = prop.def.lock().resources.get(&id).cloned().unwrap();

    let fields = diff_fields(&cur_resource.summary(), &prop_resource.summary());
    assert_eq!(
        fields,
        vec!["container".to_string()],
        "only the container (with the new image) should differ"
    );
}
