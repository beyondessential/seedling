use serde_json::{Value, json};

use super::*;
use crate::names::{AppVolumeName, SiteServiceName, SiteVolumeName};

fn test_actor() -> Arc<Actor> {
    Arc::new(Actor {
        kind: Some("ctl".into()),
        id: Some("fp123".into()),
        display: Some("Test Operator".into()),
        session: Some("sess-1".into()),
    })
}

fn capture(f: impl FnOnce(&EventSender)) -> Value {
    let tx = new_event_channel();
    let mut rx = tx.subscribe();
    f(&tx);
    let event = rx.try_recv().expect("exactly one event emitted");
    assert!(rx.try_recv().is_err(), "more than one event emitted");
    serde_json::to_value(&event).unwrap()
}

/// Capture a single emitted event and strip its timestamp (asserting one was
/// present) so tests can compare the rest of the wire shape exactly.
fn shape(f: impl FnOnce(&EventSender)) -> Value {
    let mut v = capture(f);
    let ts = v
        .as_object_mut()
        .unwrap()
        .remove("timestamp")
        .expect("event carries a timestamp");
    assert!(ts.is_string());
    v
}

fn app() -> AppName {
    AppName::new("web").unwrap()
}

// i[verify event.types]
#[test]
fn events_carry_rfc3339_timestamp() {
    let v = capture(|tx| tx.server_busy("draining"));
    let ts = v["timestamp"].as_str().expect("timestamp is a string");
    ts.parse::<Timestamp>()
        .expect("timestamp parses as RFC 3339");
}

#[test]
fn emit_without_subscribers_is_dropped_silently() {
    let tx = new_event_channel();
    tx.server_busy("nobody listening");
}

// i[verify event.types]
// r[verify audit.log.generations]
#[test]
fn app_lifecycle_events_wire_shape() {
    let app = app();

    assert_eq!(
        shape(|tx| tx.app_registered(&app, 1, Some(test_actor()))),
        json!({
            "type": "AppRegistered",
            "app": "web",
            "generation": 1,
            "actor": {
                "kind": "ctl",
                "id": "fp123",
                "display": "Test Operator",
                "session": "sess-1",
            },
        })
    );

    assert_eq!(
        shape(|tx| tx.app_deregistered(&app, None)),
        json!({"type": "AppDeregistered", "app": "web"})
    );

    assert_eq!(
        shape(|tx| tx.app_updated(&app, 3, Some(2), None)),
        json!({
            "type": "AppUpdated",
            "app": "web",
            "generation": 3,
            "previous_generation": 2,
        })
    );

    assert_eq!(
        shape(|tx| tx.app_updated(&app, 1, None, None)),
        json!({
            "type": "AppUpdated",
            "app": "web",
            "generation": 1,
            "previous_generation": null,
        })
    );

    assert_eq!(
        shape(|tx| tx.app_phase_changed(&app, "installing", None)),
        json!({"type": "AppPhaseChanged", "app": "web", "phase": "installing"})
    );
}

// i[verify event.types]
// i[verify param.store.secret]
// r[verify audit.log.generations]
#[test]
fn param_events_wire_shape() {
    let app = app();
    let name = ParamName::new("db-url").unwrap();

    assert_eq!(
        shape(|tx| tx
            .param_change(app.clone(), 5, 4, None)
            .set(&name, Some("old"), "new")),
        json!({
            "type": "ParamSet",
            "app": "web",
            "name": "db-url",
            "previous_value": "old",
            "new_value": "new",
            "generation": 5,
            "previous_generation": 4,
        })
    );

    assert_eq!(
        shape(|tx| tx
            .param_change(app.clone(), 2, 1, None)
            .set(&name, None, "v")),
        json!({
            "type": "ParamSet",
            "app": "web",
            "name": "db-url",
            "new_value": "v",
            "generation": 2,
            "previous_generation": 1,
        })
    );

    assert_eq!(
        shape(|tx| tx.param_change(app.clone(), 5, 4, None).set_redacted(&name)),
        json!({
            "type": "ParamSet",
            "app": "web",
            "name": "db-url",
            "redacted": true,
            "generation": 5,
            "previous_generation": 4,
        })
    );

    assert_eq!(
        shape(|tx| tx.param_change(app.clone(), 6, 5, None).unset(&name, "old")),
        json!({
            "type": "ParamUnset",
            "app": "web",
            "name": "db-url",
            "previous_value": "old",
            "generation": 6,
            "previous_generation": 5,
        })
    );

    assert_eq!(
        shape(|tx| tx
            .param_change(app.clone(), 6, 5, None)
            .unset_redacted(&name)),
        json!({
            "type": "ParamUnset",
            "app": "web",
            "name": "db-url",
            "redacted": true,
            "generation": 6,
            "previous_generation": 5,
        })
    );
}

// i[verify event.types]
// r[verify operation.lifecycle.generations]
#[test]
fn operation_lifecycle_events_wire_shape() {
    let app = app();
    let action = ActionName::new("start").unwrap();

    assert_eq!(
        shape(|tx| tx
            .operation(app.clone(), action.clone(), "op-1", 2, 3, None)
            .started("param_change")),
        json!({
            "type": "OperationStarted",
            "app": "web",
            "action_name": "start",
            "operation_id": "op-1",
            "source_generation": 2,
            "target_generation": 3,
            "trigger": "param_change",
        })
    );

    assert_eq!(
        shape(|tx| tx
            .operation(app.clone(), action.clone(), "op-1", 2, 3, None)
            .completed()),
        json!({
            "type": "OperationCompleted",
            "app": "web",
            "action_name": "start",
            "operation_id": "op-1",
            "source_generation": 2,
            "target_generation": 3,
        })
    );

    assert_eq!(
        shape(|tx| tx
            .operation(app.clone(), action.clone(), "op-1", 2, 3, None)
            .failed("container exploded")),
        json!({
            "type": "OperationFailed",
            "app": "web",
            "action_name": "start",
            "operation_id": "op-1",
            "source_generation": 2,
            "target_generation": 3,
            "error": "container exploded",
        })
    );
}

// i[verify event.types]
#[test]
fn fault_and_resource_events_wire_shape() {
    let app = app();

    assert_eq!(
        shape(|tx| tx.fault_filed(
            "f1",
            &app,
            Some("deployment"),
            Some("srv"),
            Some("srv-0"),
            "health_check_failed",
            "no response",
        )),
        json!({
            "type": "FaultFiled",
            "id": "f1",
            "app": "web",
            "resource_type": "deployment",
            "resource_name": "srv",
            "instance_id": "srv-0",
            "kind": "health_check_failed",
            "description": "no response",
        })
    );

    assert_eq!(
        shape(|tx| tx.fault_filed("f2", &app, None, None, None, "script_error", "boom")),
        json!({
            "type": "FaultFiled",
            "id": "f2",
            "app": "web",
            "resource_type": null,
            "resource_name": null,
            "instance_id": null,
            "kind": "script_error",
            "description": "boom",
        })
    );

    assert_eq!(
        shape(|tx| tx.fault_cleared("f1", &app, "health_check_failed")),
        json!({
            "type": "FaultCleared",
            "id": "f1",
            "app": "web",
            "kind": "health_check_failed",
        })
    );

    assert_eq!(
        shape(|tx| tx.resource_state_changed(&app, "deployment", "srv", "srv-0", "running")),
        json!({
            "type": "ResourceStateChanged",
            "app": "web",
            "resource_type": "deployment",
            "resource_name": "srv",
            "instance_id": "srv-0",
            "state": "running",
        })
    );
}

// i[verify shell.start]
// i[verify shell.exit]
#[test]
fn shell_events_wire_shape() {
    let app = app();
    let name = ShellName::new("psql").unwrap();
    let session = SessionId::generate();

    assert_eq!(
        shape(|tx| tx.shell_started(session, &app, &name)),
        json!({
            "type": "ShellStarted",
            "session_id": session.to_string(),
            "app": "web",
            "name": "psql",
        })
    );

    assert_eq!(
        shape(|tx| tx.shell_exited(session, 130)),
        json!({
            "type": "ShellExited",
            "session_id": session.to_string(),
            "exit_code": 130,
        })
    );
}

// i[verify forward.start]
#[test]
fn forward_events_wire_shape() {
    let app = app();
    let forward = ForwardId::generate();

    assert_eq!(
        shape(|tx| tx.forward_started(forward, &app, "api", 8080)),
        json!({
            "type": "ForwardStarted",
            "forward_id": forward.to_string(),
            "app": "web",
            "service": "api",
            "port": 8080,
        })
    );

    assert_eq!(
        shape(|tx| tx.forward_stopped(forward)),
        json!({"type": "ForwardStopped", "forward_id": forward.to_string()})
    );
}

// i[verify event.types]
#[test]
fn scale_and_server_busy_events_wire_shape() {
    let app = app();

    assert_eq!(
        shape(|tx| tx.scale(app.clone(), "srv", 1, 5, None).changed(3, 1)),
        json!({
            "type": "ScaleChanged",
            "app": "web",
            "deployment": "srv",
            "scale": 3,
            "previous_scale": 1,
            "bounds_low": 1,
            "bounds_high": 5,
        })
    );

    assert_eq!(
        shape(|tx| tx.server_busy("replay in progress")),
        json!({"type": "ServerBusy", "reason": "replay in progress"})
    );
}

// i[verify deployment.restart]
// i[verify resource.stop]
// i[verify resource.unstop]
#[test]
fn deployment_restart_and_resource_stop_events_wire_shape() {
    let app = app();

    assert_eq!(
        shape(|tx| tx.deployment_restarted(&app, "srv", "op-9", None)),
        json!({
            "type": "DeploymentRestarted",
            "app": "web",
            "deployment": "srv",
            "operation_id": "op-9",
        })
    );

    assert_eq!(
        shape(|tx| tx.resource_stopped(&app, "deployment", "srv", None)),
        json!({
            "type": "ResourceStopped",
            "app": "web",
            "kind": "deployment",
            "name": "srv",
        })
    );

    assert_eq!(
        shape(|tx| tx.resource_unstopped(&app, "deployment", "srv", None)),
        json!({
            "type": "ResourceUnstopped",
            "app": "web",
            "kind": "deployment",
            "name": "srv",
        })
    );
}

// r[verify actuate.volume.hold.events]
#[test]
fn held_volume_events_wire_shape() {
    let app = app();
    let held = HeldVolumeId::generate();

    assert_eq!(
        shape(|tx| tx.held_volume_created(held, &app, "data", "uninstall", None)),
        json!({
            "type": "HeldVolumeCreated",
            "held_id": held.to_string(),
            "app": "web",
            "volume_name": "data",
            "reason": "uninstall",
        })
    );

    assert_eq!(
        shape(|tx| tx.held_volume_deleted(held, None)),
        json!({"type": "HeldVolumeDeleted", "held_id": held.to_string()})
    );

    assert_eq!(
        shape(|tx| tx.held_volume_restored(held, "recovered-data", None)),
        json!({
            "type": "HeldVolumeRestored",
            "held_id": held.to_string(),
            "site_name": "recovered-data",
        })
    );
}

// r[verify volume.site.lifecycle.events]
#[test]
fn site_volume_lifecycle_events_wire_shape() {
    let held = HeldVolumeId::generate();

    assert_eq!(
        shape(|tx| tx.site_volume_created("backups", "managed", None, None)),
        json!({"type": "SiteVolumeCreated", "name": "backups", "kind": "managed"})
    );

    assert_eq!(
        shape(|tx| tx.site_volume_created("media", "bind", Some("/srv/media"), None)),
        json!({
            "type": "SiteVolumeCreated",
            "name": "media",
            "kind": "bind",
            "host_path": "/srv/media",
        })
    );

    assert_eq!(
        shape(|tx| tx.site_volume_deleted("backups", "managed", Some(held), None)),
        json!({
            "type": "SiteVolumeDeleted",
            "name": "backups",
            "kind": "managed",
            "held_id": held.to_string(),
        })
    );

    assert_eq!(
        shape(|tx| tx.site_volume_deleted("media", "bind", None, None)),
        json!({"type": "SiteVolumeDeleted", "name": "media", "kind": "bind"})
    );
}

// r[verify volume.site.snapshot.events]
// r[verify volume.site.promote.events]
#[test]
fn site_volume_snapshot_and_promote_events_wire_shape() {
    let source = VolumeRef::App {
        app: app(),
        volume: AppVolumeName::new("data").unwrap(),
    };

    assert_eq!(
        shape(|tx| tx.site_volume_snapshotted("snap-1", &source, None)),
        json!({
            "type": "SiteVolumeSnapshotted",
            "name": "snap-1",
            "source": {"kind": "app", "app": "web", "volume": "data"},
        })
    );

    assert_eq!(
        shape(|tx| tx.site_volume_promoted("restored", "snap-1", None)),
        json!({
            "type": "SiteVolumePromoted",
            "name": "restored",
            "source": "snap-1",
        })
    );
}

// r[verify volume.external.mapping.events]
#[test]
fn external_volume_mapping_events_wire_shape() {
    let app = app();
    let slot = ExternalVolumeName::new("data").unwrap();
    let site_target = VolumeRef::Site {
        name: SiteVolumeName::new("shared").unwrap(),
    };
    let previous_target = VolumeRef::Site {
        name: SiteVolumeName::new("old-shared").unwrap(),
    };

    assert_eq!(
        shape(|tx| tx.external_volume_mapped(&app, &slot, &site_target, true, None)),
        json!({
            "type": "ExternalVolumeMapped",
            "app": "web",
            "external_name": "data",
            "target": {"kind": "site", "name": "shared"},
            "read_only": true,
        })
    );

    assert_eq!(
        shape(|tx| tx.external_volume_unmapped(&app, &slot, None)),
        json!({
            "type": "ExternalVolumeUnmapped",
            "app": "web",
            "external_name": "data",
        })
    );

    assert_eq!(
        shape(|tx| tx.external_volume_remapped(
            &app,
            &slot,
            ExternalMappingSnapshot {
                target: &site_target,
                read_only: false,
            },
            ExternalMappingSnapshot {
                target: &previous_target,
                read_only: true,
            },
            None,
        )),
        json!({
            "type": "ExternalVolumeRemapped",
            "app": "web",
            "external_name": "data",
            "target": {"kind": "site", "name": "shared"},
            "read_only": false,
            "previous_target": {"kind": "site", "name": "old-shared"},
            "previous_read_only": true,
        })
    );
}

// r[verify service.site.lifecycle.events]
#[test]
fn site_service_events_wire_shape() {
    assert_eq!(
        shape(|tx| tx.site_service_created("postgres", Some("prod database"), None)),
        json!({
            "type": "SiteServiceCreated",
            "name": "postgres",
            "description": "prod database",
        })
    );

    assert_eq!(
        shape(|tx| tx.site_service_created("redis", None, None)),
        json!({"type": "SiteServiceCreated", "name": "redis"})
    );

    assert_eq!(
        shape(|tx| tx.site_service_deleted("postgres", None)),
        json!({"type": "SiteServiceDeleted", "name": "postgres"})
    );

    assert_eq!(
        shape(|tx| tx.site_service_endpoint_added("postgres", 5432, "tcp", "10.0.0.1", 5433, None)),
        json!({
            "type": "SiteServiceEndpointAdded",
            "name": "postgres",
            "service_port": 5432,
            "protocol": "tcp",
            "remote_host": "10.0.0.1",
            "remote_port": 5433,
        })
    );

    assert_eq!(
        shape(
            |tx| tx.site_service_endpoint_removed("postgres", 5432, "tcp", "10.0.0.1", 5433, None)
        ),
        json!({
            "type": "SiteServiceEndpointRemoved",
            "name": "postgres",
            "service_port": 5432,
            "protocol": "tcp",
            "remote_host": "10.0.0.1",
            "remote_port": 5433,
        })
    );
}

// r[verify service.external.mapping.events]
#[test]
fn external_service_mapping_events_wire_shape() {
    let app = app();
    let slot = ExternalServiceName::new("upstream").unwrap();
    let target = ServiceRef::Site {
        name: SiteServiceName::new("postgres").unwrap(),
    };
    let previous = ServiceRef::App {
        app: AppName::new("legacy").unwrap(),
        service: AppServiceName::new("api").unwrap(),
    };

    assert_eq!(
        shape(|tx| tx.external_service_mapped(&app, &slot, &target, None)),
        json!({
            "type": "ExternalServiceMapped",
            "app": "web",
            "external_name": "upstream",
            "target": {"kind": "site", "name": "postgres"},
        })
    );

    assert_eq!(
        shape(|tx| tx.external_service_unmapped(&app, &slot, None)),
        json!({
            "type": "ExternalServiceUnmapped",
            "app": "web",
            "external_name": "upstream",
        })
    );

    assert_eq!(
        shape(|tx| tx.external_service_remapped(
            &app,
            &slot,
            ExternalServiceMappingSnapshot { target: &target },
            ExternalServiceMappingSnapshot { target: &previous },
            None,
        )),
        json!({
            "type": "ExternalServiceRemapped",
            "app": "web",
            "external_name": "upstream",
            "target": {"kind": "site", "name": "postgres"},
            "previous_target": {"kind": "app", "app": "legacy", "service": "api"},
        })
    );
}

// r[verify ingress.site.lifecycle.events]
#[test]
fn site_ingress_events_wire_shape() {
    let app = app();
    let name = SiteIngressName::new("front").unwrap();
    let service = AppServiceName::new("api").unwrap();

    assert_eq!(
        shape(|tx| tx.site_ingress_created(
            &name,
            "example.com",
            "manual",
            None,
            "acme",
            Some("public entry"),
            None,
        )),
        json!({
            "type": "SiteIngressCreated",
            "name": "front",
            "hostname": "example.com",
            "source": "manual",
            "tls_provider": "acme",
            "description": "public entry",
        })
    );

    assert_eq!(
        shape(|tx| tx.site_ingress_created(
            &name,
            "host.tail.net",
            "discovered",
            Some("tailscale"),
            "tailscale",
            None,
            None,
        )),
        json!({
            "type": "SiteIngressCreated",
            "name": "front",
            "hostname": "host.tail.net",
            "source": "discovered",
            "discovered_provider": "tailscale",
            "tls_provider": "tailscale",
        })
    );

    assert_eq!(
        shape(|tx| tx.site_ingress_updated(&name, "example.org", "internal", None, None)),
        json!({
            "type": "SiteIngressUpdated",
            "name": "front",
            "hostname": "example.org",
            "tls_provider": "internal",
        })
    );

    assert_eq!(
        shape(|tx| tx.site_ingress_deleted(&name, "manual", None)),
        json!({"type": "SiteIngressDeleted", "name": "front", "source": "manual"})
    );

    assert_eq!(
        shape(|tx| tx.site_ingress_attachment_added(
            &name,
            443,
            "https",
            "forward",
            Some(&app),
            Some(&service),
            None,
            None,
            None,
        )),
        json!({
            "type": "SiteIngressAttachmentAdded",
            "name": "front",
            "port": 443,
            "protocol": "https",
            "target_kind": "forward",
            "target_app": "web",
            "target_service": "api",
        })
    );

    assert_eq!(
        shape(|tx| tx.site_ingress_attachment_updated(
            &name,
            443,
            "https",
            "redirect",
            None,
            None,
            Some("https://example.org/"),
            Some(301),
            None,
        )),
        json!({
            "type": "SiteIngressAttachmentUpdated",
            "name": "front",
            "port": 443,
            "protocol": "https",
            "target_kind": "redirect",
            "redirect_url": "https://example.org/",
            "redirect_code": 301,
        })
    );

    assert_eq!(
        shape(|tx| tx.site_ingress_attachment_removed(&name, 443, "https", None)),
        json!({
            "type": "SiteIngressAttachmentRemoved",
            "name": "front",
            "port": 443,
            "protocol": "https",
        })
    );
}

// r[verify audit.log.events]
#[test]
fn template_events_wire_shape() {
    let app = app();
    let name = TemplateName::new("postgres-stack").unwrap();

    assert_eq!(
        shape(|tx| tx.template_created(&name, None)),
        json!({"type": "TemplateCreated", "name": "postgres-stack"})
    );

    assert_eq!(
        shape(|tx| tx.template_updated(&name, None)),
        json!({"type": "TemplateUpdated", "name": "postgres-stack"})
    );

    assert_eq!(
        shape(|tx| tx.template_removed(&name, None)),
        json!({"type": "TemplateRemoved", "name": "postgres-stack"})
    );

    assert_eq!(
        shape(|tx| tx.template_instantiated(&name, &app, None)),
        json!({
            "type": "TemplateInstantiated",
            "template": "postgres-stack",
            "app": "web",
        })
    );
}

// i[verify wire.actor]
#[test]
fn event_sender_with_actor_attaches_actor_to_every_event() {
    let tx = new_event_channel();
    let mut rx = tx.subscribe();
    let sender = EventSenderWithActor::new(tx, test_actor());

    let app = app();
    let action = ActionName::new("start").unwrap();
    let param = ParamName::new("db-url").unwrap();
    let ext_vol = ExternalVolumeName::new("data").unwrap();
    let ext_svc = ExternalServiceName::new("upstream").unwrap();
    let ingress = SiteIngressName::new("front").unwrap();
    let template = TemplateName::new("postgres-stack").unwrap();
    let vol_ref = VolumeRef::Site {
        name: SiteVolumeName::new("shared").unwrap(),
    };
    let svc_ref = ServiceRef::Site {
        name: SiteServiceName::new("postgres").unwrap(),
    };
    let held = HeldVolumeId::generate();

    sender.app_registered(&app, 1);
    sender.app_deregistered(&app);
    sender.app_updated(&app, 2, Some(1));
    sender.app_phase_changed(&app, "installed");
    sender.scale(app.clone(), "srv", 0, 4).changed(2, 1);
    sender
        .operation(app.clone(), action.clone(), "op-1", 1, 1)
        .started("operator");
    sender
        .param_change(app.clone(), 2, 1)
        .set(&param, None, "v");
    sender.deployment_restarted(&app, "srv", "op-2");
    sender.resource_stopped(&app, "deployment", "srv");
    sender.resource_unstopped(&app, "deployment", "srv");
    sender.held_volume_created(held, &app, "data", "uninstall");
    sender.held_volume_deleted(held);
    sender.held_volume_restored(held, "recovered");
    sender.site_volume_created("shared", "managed", None);
    sender.site_volume_deleted("shared", "managed", Some(held));
    sender.site_volume_snapshotted("snap-1", &vol_ref);
    sender.site_volume_promoted("restored", "snap-1");
    sender.external_volume_mapped(&app, &ext_vol, &vol_ref, false);
    sender.external_volume_unmapped(&app, &ext_vol);
    sender.external_volume_remapped(
        &app,
        &ext_vol,
        ExternalMappingSnapshot {
            target: &vol_ref,
            read_only: true,
        },
        ExternalMappingSnapshot {
            target: &vol_ref,
            read_only: false,
        },
    );
    sender.site_service_created("postgres", None);
    sender.site_service_deleted("postgres");
    sender.site_service_endpoint_added("postgres", 5432, "tcp", "10.0.0.1", 5432);
    sender.site_service_endpoint_removed("postgres", 5432, "tcp", "10.0.0.1", 5432);
    sender.external_service_mapped(&app, &ext_svc, &svc_ref);
    sender.external_service_unmapped(&app, &ext_svc);
    sender.external_service_remapped(
        &app,
        &ext_svc,
        ExternalServiceMappingSnapshot { target: &svc_ref },
        ExternalServiceMappingSnapshot { target: &svc_ref },
    );
    sender.site_ingress_created(&ingress, "example.com", "manual", None, "acme", None);
    sender.site_ingress_updated(&ingress, "example.com", "acme", None);
    sender.site_ingress_deleted(&ingress, "manual");
    sender.site_ingress_attachment_added(
        &ingress,
        443,
        "https",
        "forward",
        Some(&app),
        None,
        None,
        None,
    );
    sender.site_ingress_attachment_updated(
        &ingress,
        443,
        "https",
        "redirect",
        None,
        None,
        Some("https://example.org/"),
        Some(301),
    );
    sender.site_ingress_attachment_removed(&ingress, 443, "https");
    sender.template_created(&template);
    sender.template_updated(&template);
    sender.template_removed(&template);
    sender.template_instantiated(&template, &app);

    let mut count = 0usize;
    while let Ok(event) = rx.try_recv() {
        let v = serde_json::to_value(&event).unwrap();
        assert_eq!(
            v["actor"]["id"], "fp123",
            "actor not attached on {} event",
            v["type"]
        );
        count += 1;
    }
    assert_eq!(count, 37, "every delegate emitted exactly one event");
}
