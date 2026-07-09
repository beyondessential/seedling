use std::sync::Arc;

use jiff::Timestamp;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::{
    actor::Actor,
    names::{
        ActionName, AppName, AppServiceName, ExternalServiceName, ExternalVolumeName, ForwardId,
        HeldVolumeId, ParamName, ServiceRef, SessionId, ShellName, SiteIngressName, TemplateName,
        VolumeRef,
    },
};

// i[event.types]
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum OiEvent {
    // r[impl audit.log.generations]
    AppRegistered {
        timestamp: Timestamp,
        app: AppName,
        generation: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    AppDeregistered {
        timestamp: Timestamp,
        app: AppName,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl audit.log.generations]
    AppUpdated {
        timestamp: Timestamp,
        app: AppName,
        generation: u64,
        previous_generation: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // i[impl event.types]
    /// Emitted on every transition of `AppPhase`. Phase is one of
    /// `"not_installed"`, `"installing"`, `"installed"`, `"uninstalling"`.
    /// The WebUI relies on this to refresh after uninstall completes,
    /// because uninstall is not driven by a BSL operation and therefore
    /// does not emit OperationStarted/OperationCompleted.
    AppPhaseChanged {
        timestamp: Timestamp,
        app: AppName,
        phase: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl audit.log.generations]
    // i[impl param.store.secret]
    ParamSet {
        timestamp: Timestamp,
        app: AppName,
        name: ParamName,
        #[serde(skip_serializing_if = "Option::is_none")]
        previous_value: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        new_value: Option<String>,
        #[serde(skip_serializing_if = "is_false")]
        redacted: bool,
        generation: u64,
        previous_generation: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl audit.log.generations]
    // i[impl param.store.secret]
    ParamUnset {
        timestamp: Timestamp,
        app: AppName,
        name: ParamName,
        #[serde(skip_serializing_if = "Option::is_none")]
        previous_value: Option<String>,
        #[serde(skip_serializing_if = "is_false")]
        redacted: bool,
        generation: u64,
        previous_generation: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl operation.lifecycle.generations]
    // i[impl event.types]
    OperationStarted {
        timestamp: Timestamp,
        app: AppName,
        action_name: ActionName,
        operation_id: String,
        source_generation: u64,
        target_generation: u64,
        trigger: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl operation.lifecycle.generations]
    OperationCompleted {
        timestamp: Timestamp,
        app: AppName,
        action_name: ActionName,
        operation_id: String,
        source_generation: u64,
        target_generation: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl operation.lifecycle.generations]
    OperationFailed {
        timestamp: Timestamp,
        app: AppName,
        action_name: ActionName,
        operation_id: String,
        source_generation: u64,
        target_generation: u64,
        error: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    FaultFiled {
        timestamp: Timestamp,
        id: String,
        app: AppName,
        resource_type: Option<String>,
        resource_name: Option<String>,
        instance_id: Option<String>,
        kind: String,
        description: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    FaultCleared {
        timestamp: Timestamp,
        id: String,
        app: AppName,
        /// Kind of the fault that was cleared. Mirrors
        /// [`FaultFiled::kind`] so UIs can render a useful summary without
        /// having to remember every fault ID they saw.
        kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    ResourceStateChanged {
        timestamp: Timestamp,
        app: AppName,
        resource_type: String,
        resource_name: String,
        instance_id: String,
        state: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    ShellStarted {
        timestamp: Timestamp,
        session_id: SessionId,
        app: AppName,
        name: ShellName,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    ShellExited {
        timestamp: Timestamp,
        session_id: SessionId,
        exit_code: i32,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    ForwardStarted {
        timestamp: Timestamp,
        forward_id: ForwardId,
        app: AppName,
        service: String,
        port: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    ForwardStopped {
        timestamp: Timestamp,
        forward_id: ForwardId,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    ScaleChanged {
        timestamp: Timestamp,
        app: AppName,
        deployment: String,
        scale: u16,
        previous_scale: u16,
        bounds_low: u16,
        bounds_high: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // i[impl deployment.restart]
    DeploymentRestarted {
        timestamp: Timestamp,
        app: AppName,
        deployment: String,
        operation_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // i[impl resource.stop]
    ResourceStopped {
        timestamp: Timestamp,
        app: AppName,
        kind: String,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // i[impl resource.unstop]
    ResourceUnstopped {
        timestamp: Timestamp,
        app: AppName,
        kind: String,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    ServerBusy {
        timestamp: Timestamp,
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    HeldVolumeCreated {
        timestamp: Timestamp,
        held_id: HeldVolumeId,
        app: AppName,
        volume_name: String,
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    HeldVolumeDeleted {
        timestamp: Timestamp,
        held_id: HeldVolumeId,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl actuate.volume.hold.events]
    HeldVolumeRestored {
        timestamp: Timestamp,
        held_id: HeldVolumeId,
        /// Name of the new managed site volume the held data became.
        site_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl volume.site.lifecycle.events]
    SiteVolumeCreated {
        timestamp: Timestamp,
        name: String,
        /// "managed" or "bind".
        kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        host_path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl volume.site.lifecycle.events]
    SiteVolumeDeleted {
        timestamp: Timestamp,
        name: String,
        /// "managed", "bind", or "snapshot".
        kind: String,
        /// Set when the deletion routed through the held-volume mechanism;
        /// absent for bind site volumes whose host path is left untouched.
        #[serde(skip_serializing_if = "Option::is_none")]
        held_id: Option<HeldVolumeId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl volume.site.snapshot.events]
    SiteVolumeSnapshotted {
        timestamp: Timestamp,
        name: String,
        source: VolumeRef,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl volume.site.promote.events]
    SiteVolumePromoted {
        timestamp: Timestamp,
        name: String,
        source: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl volume.external.mapping.events]
    ExternalVolumeMapped {
        timestamp: Timestamp,
        app: AppName,
        external_name: ExternalVolumeName,
        target: VolumeRef,
        read_only: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl volume.external.mapping.events]
    ExternalVolumeUnmapped {
        timestamp: Timestamp,
        app: AppName,
        external_name: ExternalVolumeName,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl volume.external.mapping.events]
    ExternalVolumeRemapped {
        timestamp: Timestamp,
        app: AppName,
        external_name: ExternalVolumeName,
        target: VolumeRef,
        read_only: bool,
        previous_target: VolumeRef,
        previous_read_only: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl service.site.lifecycle.events]
    SiteServiceCreated {
        timestamp: Timestamp,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl service.site.lifecycle.events]
    SiteServiceDeleted {
        timestamp: Timestamp,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl service.site.lifecycle.events]
    SiteServiceEndpointAdded {
        timestamp: Timestamp,
        name: String,
        service_port: u16,
        protocol: String,
        remote_host: String,
        remote_port: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl service.site.lifecycle.events]
    SiteServiceEndpointRemoved {
        timestamp: Timestamp,
        name: String,
        service_port: u16,
        protocol: String,
        remote_host: String,
        remote_port: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl service.external.mapping.events]
    ExternalServiceMapped {
        timestamp: Timestamp,
        app: AppName,
        external_name: ExternalServiceName,
        target: ServiceRef,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl service.external.mapping.events]
    ExternalServiceUnmapped {
        timestamp: Timestamp,
        app: AppName,
        external_name: ExternalServiceName,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl service.external.mapping.events]
    ExternalServiceRemapped {
        timestamp: Timestamp,
        app: AppName,
        external_name: ExternalServiceName,
        target: ServiceRef,
        previous_target: ServiceRef,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl ingress.site.lifecycle.events]
    SiteIngressCreated {
        timestamp: Timestamp,
        name: SiteIngressName,
        hostname: String,
        /// "manual" | "discovered"
        source: String,
        /// Provider id when source is "discovered" (e.g. "tailscale").
        #[serde(skip_serializing_if = "Option::is_none")]
        discovered_provider: Option<String>,
        /// "acme" | "tailscale" | "internal" | "none"
        tls_provider: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl ingress.site.lifecycle.events]
    SiteIngressUpdated {
        timestamp: Timestamp,
        name: SiteIngressName,
        /// New hostname (may be unchanged from prior state).
        hostname: String,
        /// New TLS provider (may be unchanged from prior state).
        tls_provider: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl ingress.site.lifecycle.events]
    SiteIngressDeleted {
        timestamp: Timestamp,
        name: SiteIngressName,
        /// "manual" | "discovered"
        source: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl ingress.site.lifecycle.events]
    SiteIngressAttachmentAdded {
        timestamp: Timestamp,
        name: SiteIngressName,
        port: u16,
        protocol: String,
        /// "forward" | "redirect"
        target_kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_app: Option<AppName>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_service: Option<AppServiceName>,
        #[serde(skip_serializing_if = "Option::is_none")]
        redirect_url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        redirect_code: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl ingress.site.lifecycle.events]
    SiteIngressAttachmentUpdated {
        timestamp: Timestamp,
        name: SiteIngressName,
        port: u16,
        protocol: String,
        target_kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_app: Option<AppName>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_service: Option<AppServiceName>,
        #[serde(skip_serializing_if = "Option::is_none")]
        redirect_url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        redirect_code: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl ingress.site.lifecycle.events]
    SiteIngressAttachmentRemoved {
        timestamp: Timestamp,
        name: SiteIngressName,
        port: u16,
        protocol: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl audit.log.events]
    TemplateCreated {
        timestamp: Timestamp,
        name: TemplateName,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl audit.log.events]
    TemplateUpdated {
        timestamp: Timestamp,
        name: TemplateName,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl audit.log.events]
    TemplateRemoved {
        timestamp: Timestamp,
        name: TemplateName,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl audit.log.events]
    TemplateInstantiated {
        timestamp: Timestamp,
        template: TemplateName,
        app: AppName,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
}

/// Newtype over `broadcast::Sender<OiEvent>` that carries event-emission methods.
#[derive(Clone, Debug)]
pub struct EventSender(broadcast::Sender<OiEvent>);

impl std::ops::Deref for EventSender {
    type Target = broadcast::Sender<OiEvent>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub fn new_event_channel() -> EventSender {
    let (tx, _) = broadcast::channel(256);
    EventSender(tx)
}

fn now() -> Timestamp {
    Timestamp::now()
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Snapshot of an external volume mapping's target. Used to emit remap
/// events without forcing the caller to stringify fields twice.
#[derive(Clone, Debug)]
pub struct ExternalMappingSnapshot<'a> {
    pub target: &'a VolumeRef,
    pub read_only: bool,
}

/// Snapshot of an external service mapping's target, used by the remap
/// event emitter to carry both the new and previous target in a single
/// call.
#[derive(Clone, Debug)]
pub struct ExternalServiceMappingSnapshot<'a> {
    pub target: &'a ServiceRef,
}

impl EventSender {
    fn emit(&self, event: OiEvent) {
        let _ = self.0.send(event);
    }

    pub fn app_registered(&self, app: &AppName, generation: u64, actor: Option<Arc<Actor>>) {
        self.emit(OiEvent::AppRegistered {
            timestamp: now(),
            app: app.clone(),
            generation,
            actor,
        });
    }

    pub fn app_deregistered(&self, app: &AppName, actor: Option<Arc<Actor>>) {
        self.emit(OiEvent::AppDeregistered {
            timestamp: now(),
            app: app.clone(),
            actor,
        });
    }

    pub fn app_updated(
        &self,
        app: &AppName,
        generation: u64,
        previous_generation: Option<u64>,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::AppUpdated {
            timestamp: now(),
            app: app.clone(),
            generation,
            previous_generation,
            actor,
        });
    }

    /// Emit `AppPhaseChanged`. `phase` is the lowercase snake-case phase name
    /// matching [`crate::events::OiEvent::AppPhaseChanged::phase`].
    pub fn app_phase_changed(&self, app: &AppName, phase: &str, actor: Option<Arc<Actor>>) {
        self.emit(OiEvent::AppPhaseChanged {
            timestamp: now(),
            app: app.clone(),
            phase: phase.to_owned(),
            actor,
        });
    }

    /// Build a context for scale-change events.
    /// Captures the deployment identity and bounds; call `.changed(new, prev)` to emit.
    pub fn scale(
        &self,
        app: AppName,
        deployment: impl Into<String>,
        bounds_low: u16,
        bounds_high: u16,
        actor: Option<Arc<Actor>>,
    ) -> ScaleEventCtx {
        ScaleEventCtx {
            tx: self.clone(),
            app,
            deployment: deployment.into(),
            bounds_low,
            bounds_high,
            actor,
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "three optional resource qualifier fields mirror OiEvent::FaultFiled and cannot be meaningfully collapsed"
    )]
    pub fn fault_filed(
        &self,
        id: &str,
        app: &AppName,
        resource_type: Option<&str>,
        resource_name: Option<&str>,
        instance_id: Option<&str>,
        kind: &str,
        description: &str,
    ) {
        self.emit(OiEvent::FaultFiled {
            timestamp: now(),
            id: id.to_owned(),
            app: app.clone(),
            resource_type: resource_type.map(str::to_owned),
            resource_name: resource_name.map(str::to_owned),
            instance_id: instance_id.map(str::to_owned),
            kind: kind.to_owned(),
            description: description.to_owned(),
            actor: None,
        });
    }

    pub fn fault_cleared(&self, id: &str, app: &AppName, kind: &str) {
        self.emit(OiEvent::FaultCleared {
            timestamp: now(),
            id: id.to_owned(),
            app: app.clone(),
            kind: kind.to_owned(),
            actor: None,
        });
    }

    pub fn resource_state_changed(
        &self,
        app: &AppName,
        resource_type: &str,
        resource_name: &str,
        instance_id: &str,
        state: &str,
    ) {
        self.emit(OiEvent::ResourceStateChanged {
            timestamp: now(),
            app: app.clone(),
            resource_type: resource_type.to_owned(),
            resource_name: resource_name.to_owned(),
            instance_id: instance_id.to_owned(),
            state: state.to_owned(),
            actor: None,
        });
    }

    // i[impl shell.start]
    pub fn shell_started(&self, session_id: SessionId, app: &AppName, name: &ShellName) {
        self.emit(OiEvent::ShellStarted {
            timestamp: now(),
            session_id,
            app: app.clone(),
            name: name.clone(),
            actor: None,
        });
    }

    // i[impl shell.exit]
    pub fn shell_exited(&self, session_id: SessionId, exit_code: i32) {
        self.emit(OiEvent::ShellExited {
            timestamp: now(),
            session_id,
            exit_code,
            actor: None,
        });
    }

    // i[impl forward.start]
    pub fn forward_started(&self, forward_id: ForwardId, app: &AppName, service: &str, port: u16) {
        self.emit(OiEvent::ForwardStarted {
            timestamp: now(),
            forward_id,
            app: app.clone(),
            service: service.to_owned(),
            port,
            actor: None,
        });
    }

    // i[impl forward.start]
    pub fn forward_stopped(&self, forward_id: ForwardId) {
        self.emit(OiEvent::ForwardStopped {
            timestamp: now(),
            forward_id,
            actor: None,
        });
    }

    pub fn server_busy(&self, reason: &str) {
        self.emit(OiEvent::ServerBusy {
            timestamp: now(),
            reason: reason.to_owned(),
            actor: None,
        });
    }

    // r[impl actuate.volume.hold.events]
    pub fn held_volume_created(
        &self,
        held_id: HeldVolumeId,
        app: &AppName,
        volume_name: &str,
        reason: &str,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::HeldVolumeCreated {
            timestamp: now(),
            held_id,
            app: app.clone(),
            volume_name: volume_name.to_owned(),
            reason: reason.to_owned(),
            actor,
        });
    }

    // r[impl actuate.volume.hold.events]
    pub fn held_volume_deleted(&self, held_id: HeldVolumeId, actor: Option<Arc<Actor>>) {
        self.emit(OiEvent::HeldVolumeDeleted {
            timestamp: now(),
            held_id,
            actor,
        });
    }

    // r[impl actuate.volume.hold.events]
    pub fn held_volume_restored(
        &self,
        held_id: HeldVolumeId,
        site_name: &str,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::HeldVolumeRestored {
            timestamp: now(),
            held_id,
            site_name: site_name.to_owned(),
            actor,
        });
    }

    // r[impl volume.site.lifecycle.events]
    pub fn site_volume_created(
        &self,
        name: &str,
        kind: &str,
        host_path: Option<&str>,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::SiteVolumeCreated {
            timestamp: now(),
            name: name.to_owned(),
            kind: kind.to_owned(),
            host_path: host_path.map(str::to_owned),
            actor,
        });
    }

    // r[impl volume.site.lifecycle.events]
    pub fn site_volume_deleted(
        &self,
        name: &str,
        kind: &str,
        held_id: Option<HeldVolumeId>,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::SiteVolumeDeleted {
            timestamp: now(),
            name: name.to_owned(),
            kind: kind.to_owned(),
            held_id,
            actor,
        });
    }

    // r[impl volume.site.snapshot.events]
    pub fn site_volume_snapshotted(
        &self,
        name: &str,
        source: &VolumeRef,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::SiteVolumeSnapshotted {
            timestamp: now(),
            name: name.to_owned(),
            source: source.clone(),
            actor,
        });
    }

    // r[impl volume.site.promote.events]
    pub fn site_volume_promoted(&self, name: &str, source: &str, actor: Option<Arc<Actor>>) {
        self.emit(OiEvent::SiteVolumePromoted {
            timestamp: now(),
            name: name.to_owned(),
            source: source.to_owned(),
            actor,
        });
    }

    // r[impl volume.external.mapping.events]
    pub fn external_volume_mapped(
        &self,
        app: &AppName,
        external_name: &ExternalVolumeName,
        target: &VolumeRef,
        read_only: bool,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::ExternalVolumeMapped {
            timestamp: now(),
            app: app.clone(),
            external_name: external_name.clone(),
            target: target.clone(),
            read_only,
            actor,
        });
    }

    // r[impl volume.external.mapping.events]
    pub fn external_volume_unmapped(
        &self,
        app: &AppName,
        external_name: &ExternalVolumeName,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::ExternalVolumeUnmapped {
            timestamp: now(),
            app: app.clone(),
            external_name: external_name.clone(),
            actor,
        });
    }

    // r[impl volume.external.mapping.events]
    pub fn external_volume_remapped(
        &self,
        app: &AppName,
        external_name: &ExternalVolumeName,
        new: ExternalMappingSnapshot<'_>,
        previous: ExternalMappingSnapshot<'_>,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::ExternalVolumeRemapped {
            timestamp: now(),
            app: app.clone(),
            external_name: external_name.clone(),
            target: new.target.clone(),
            read_only: new.read_only,
            previous_target: previous.target.clone(),
            previous_read_only: previous.read_only,
            actor,
        });
    }

    // r[impl service.site.lifecycle.events]
    pub fn site_service_created(
        &self,
        name: &str,
        description: Option<&str>,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::SiteServiceCreated {
            timestamp: now(),
            name: name.to_owned(),
            description: description.map(str::to_owned),
            actor,
        });
    }

    // r[impl service.site.lifecycle.events]
    pub fn site_service_deleted(&self, name: &str, actor: Option<Arc<Actor>>) {
        self.emit(OiEvent::SiteServiceDeleted {
            timestamp: now(),
            name: name.to_owned(),
            actor,
        });
    }

    // r[impl service.site.lifecycle.events]
    #[allow(clippy::too_many_arguments)]
    pub fn site_service_endpoint_added(
        &self,
        name: &str,
        service_port: u16,
        protocol: &str,
        remote_host: &str,
        remote_port: u16,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::SiteServiceEndpointAdded {
            timestamp: now(),
            name: name.to_owned(),
            service_port,
            protocol: protocol.to_owned(),
            remote_host: remote_host.to_owned(),
            remote_port,
            actor,
        });
    }

    // r[impl service.site.lifecycle.events]
    #[allow(clippy::too_many_arguments)]
    pub fn site_service_endpoint_removed(
        &self,
        name: &str,
        service_port: u16,
        protocol: &str,
        remote_host: &str,
        remote_port: u16,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::SiteServiceEndpointRemoved {
            timestamp: now(),
            name: name.to_owned(),
            service_port,
            protocol: protocol.to_owned(),
            remote_host: remote_host.to_owned(),
            remote_port,
            actor,
        });
    }

    // r[impl service.external.mapping.events]
    pub fn external_service_mapped(
        &self,
        app: &AppName,
        external_name: &ExternalServiceName,
        target: &ServiceRef,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::ExternalServiceMapped {
            timestamp: now(),
            app: app.clone(),
            external_name: external_name.clone(),
            target: target.clone(),
            actor,
        });
    }

    // r[impl service.external.mapping.events]
    pub fn external_service_unmapped(
        &self,
        app: &AppName,
        external_name: &ExternalServiceName,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::ExternalServiceUnmapped {
            timestamp: now(),
            app: app.clone(),
            external_name: external_name.clone(),
            actor,
        });
    }

    // r[impl service.external.mapping.events]
    pub fn external_service_remapped(
        &self,
        app: &AppName,
        external_name: &ExternalServiceName,
        new: ExternalServiceMappingSnapshot<'_>,
        previous: ExternalServiceMappingSnapshot<'_>,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::ExternalServiceRemapped {
            timestamp: now(),
            app: app.clone(),
            external_name: external_name.clone(),
            target: new.target.clone(),
            previous_target: previous.target.clone(),
            actor,
        });
    }

    // r[impl ingress.site.lifecycle.events]
    #[allow(clippy::too_many_arguments)]
    pub fn site_ingress_created(
        &self,
        name: &SiteIngressName,
        hostname: &str,
        source: &str,
        discovered_provider: Option<&str>,
        tls_provider: &str,
        description: Option<&str>,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::SiteIngressCreated {
            timestamp: now(),
            name: name.clone(),
            hostname: hostname.to_owned(),
            source: source.to_owned(),
            discovered_provider: discovered_provider.map(str::to_owned),
            tls_provider: tls_provider.to_owned(),
            description: description.map(str::to_owned),
            actor,
        });
    }

    // r[impl ingress.site.lifecycle.events]
    pub fn site_ingress_updated(
        &self,
        name: &SiteIngressName,
        hostname: &str,
        tls_provider: &str,
        description: Option<&str>,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::SiteIngressUpdated {
            timestamp: now(),
            name: name.clone(),
            hostname: hostname.to_owned(),
            tls_provider: tls_provider.to_owned(),
            description: description.map(str::to_owned),
            actor,
        });
    }

    // r[impl ingress.site.lifecycle.events]
    pub fn site_ingress_deleted(
        &self,
        name: &SiteIngressName,
        source: &str,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::SiteIngressDeleted {
            timestamp: now(),
            name: name.clone(),
            source: source.to_owned(),
            actor,
        });
    }

    // r[impl ingress.site.lifecycle.events]
    #[allow(clippy::too_many_arguments)]
    pub fn site_ingress_attachment_added(
        &self,
        name: &SiteIngressName,
        port: u16,
        protocol: &str,
        target_kind: &str,
        target_app: Option<&AppName>,
        target_service: Option<&AppServiceName>,
        redirect_url: Option<&str>,
        redirect_code: Option<u16>,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::SiteIngressAttachmentAdded {
            timestamp: now(),
            name: name.clone(),
            port,
            protocol: protocol.to_owned(),
            target_kind: target_kind.to_owned(),
            target_app: target_app.cloned(),
            target_service: target_service.cloned(),
            redirect_url: redirect_url.map(str::to_owned),
            redirect_code,
            actor,
        });
    }

    // r[impl ingress.site.lifecycle.events]
    #[allow(clippy::too_many_arguments)]
    pub fn site_ingress_attachment_updated(
        &self,
        name: &SiteIngressName,
        port: u16,
        protocol: &str,
        target_kind: &str,
        target_app: Option<&AppName>,
        target_service: Option<&AppServiceName>,
        redirect_url: Option<&str>,
        redirect_code: Option<u16>,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::SiteIngressAttachmentUpdated {
            timestamp: now(),
            name: name.clone(),
            port,
            protocol: protocol.to_owned(),
            target_kind: target_kind.to_owned(),
            target_app: target_app.cloned(),
            target_service: target_service.cloned(),
            redirect_url: redirect_url.map(str::to_owned),
            redirect_code,
            actor,
        });
    }

    // r[impl ingress.site.lifecycle.events]
    pub fn site_ingress_attachment_removed(
        &self,
        name: &SiteIngressName,
        port: u16,
        protocol: &str,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::SiteIngressAttachmentRemoved {
            timestamp: now(),
            name: name.clone(),
            port,
            protocol: protocol.to_owned(),
            actor,
        });
    }

    // i[impl deployment.restart]
    pub fn deployment_restarted(
        &self,
        app: &AppName,
        deployment: &str,
        operation_id: &str,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::DeploymentRestarted {
            timestamp: now(),
            app: app.clone(),
            deployment: deployment.to_owned(),
            operation_id: operation_id.to_owned(),
            actor,
        });
    }

    // i[impl resource.stop]
    pub fn resource_stopped(
        &self,
        app: &AppName,
        kind: &str,
        name: &str,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::ResourceStopped {
            timestamp: now(),
            app: app.clone(),
            kind: kind.to_owned(),
            name: name.to_owned(),
            actor,
        });
    }

    // i[impl resource.unstop]
    pub fn resource_unstopped(
        &self,
        app: &AppName,
        kind: &str,
        name: &str,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::ResourceUnstopped {
            timestamp: now(),
            app: app.clone(),
            kind: kind.to_owned(),
            name: name.to_owned(),
            actor,
        });
    }

    /// Build a context for the three operation lifecycle events.
    /// The context is `Clone + Send + 'static` and can cross the blocking thread boundary.
    // i[wire.actor]
    pub fn operation(
        &self,
        app: AppName,
        action_name: ActionName,
        operation_id: impl Into<String>,
        source_generation: u64,
        target_generation: u64,
        actor: Option<Arc<Actor>>,
    ) -> OperationEventCtx {
        OperationEventCtx {
            tx: self.clone(),
            app,
            action_name,
            operation_id: operation_id.into(),
            source_generation,
            target_generation,
            actor,
        }
    }

    /// Build a context for param-change events (set/unset).
    pub fn param_change(
        &self,
        app: AppName,
        generation: u64,
        previous_generation: u64,
        actor: Option<Arc<Actor>>,
    ) -> ParamEventCtx {
        ParamEventCtx {
            tx: self.clone(),
            app,
            generation,
            previous_generation,
            actor,
        }
    }

    // r[impl audit.log.events]
    pub fn template_created(&self, name: &TemplateName, actor: Option<Arc<Actor>>) {
        self.emit(OiEvent::TemplateCreated {
            timestamp: now(),
            name: name.clone(),
            actor,
        });
    }

    // r[impl audit.log.events]
    pub fn template_updated(&self, name: &TemplateName, actor: Option<Arc<Actor>>) {
        self.emit(OiEvent::TemplateUpdated {
            timestamp: now(),
            name: name.clone(),
            actor,
        });
    }

    // r[impl audit.log.events]
    pub fn template_removed(&self, name: &TemplateName, actor: Option<Arc<Actor>>) {
        self.emit(OiEvent::TemplateRemoved {
            timestamp: now(),
            name: name.clone(),
            actor,
        });
    }

    // r[impl audit.log.events]
    pub fn template_instantiated(
        &self,
        template: &TemplateName,
        app: &AppName,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::TemplateInstantiated {
            timestamp: now(),
            template: template.clone(),
            app: app.clone(),
            actor,
        });
    }
}

/// An `EventSender` bound to a specific actor for the duration of an OI request.
/// All audit-trail event methods are available without passing actor at each call site.
#[derive(Clone, Debug)]
pub struct EventSenderWithActor {
    inner: EventSender,
    pub actor: Arc<Actor>,
}

impl EventSenderWithActor {
    pub fn new(inner: EventSender, actor: Arc<Actor>) -> Self {
        Self { inner, actor }
    }

    pub fn app_registered(&self, app: &AppName, generation: u64) {
        self.inner
            .app_registered(app, generation, Some(Arc::clone(&self.actor)));
    }

    pub fn app_deregistered(&self, app: &AppName) {
        self.inner
            .app_deregistered(app, Some(Arc::clone(&self.actor)));
    }

    pub fn app_updated(&self, app: &AppName, generation: u64, previous_generation: Option<u64>) {
        self.inner.app_updated(
            app,
            generation,
            previous_generation,
            Some(Arc::clone(&self.actor)),
        );
    }

    pub fn app_phase_changed(&self, app: &AppName, phase: &str) {
        self.inner
            .app_phase_changed(app, phase, Some(Arc::clone(&self.actor)));
    }

    pub fn scale(
        &self,
        app: AppName,
        deployment: impl Into<String>,
        bounds_low: u16,
        bounds_high: u16,
    ) -> ScaleEventCtx {
        self.inner.scale(
            app,
            deployment,
            bounds_low,
            bounds_high,
            Some(Arc::clone(&self.actor)),
        )
    }

    // i[wire.actor]
    pub fn operation(
        &self,
        app: AppName,
        action_name: ActionName,
        operation_id: impl Into<String>,
        source_generation: u64,
        target_generation: u64,
    ) -> OperationEventCtx {
        self.inner.operation(
            app,
            action_name,
            operation_id,
            source_generation,
            target_generation,
            Some(Arc::clone(&self.actor)),
        )
    }

    pub fn param_change(
        &self,
        app: AppName,
        generation: u64,
        previous_generation: u64,
    ) -> ParamEventCtx {
        self.inner.param_change(
            app,
            generation,
            previous_generation,
            Some(Arc::clone(&self.actor)),
        )
    }

    // i[impl deployment.restart]
    pub fn deployment_restarted(&self, app: &AppName, deployment: &str, operation_id: &str) {
        self.inner.deployment_restarted(
            app,
            deployment,
            operation_id,
            Some(Arc::clone(&self.actor)),
        );
    }

    // i[impl resource.stop]
    pub fn resource_stopped(&self, app: &AppName, kind: &str, name: &str) {
        self.inner
            .resource_stopped(app, kind, name, Some(Arc::clone(&self.actor)));
    }

    // i[impl resource.unstop]
    pub fn resource_unstopped(&self, app: &AppName, kind: &str, name: &str) {
        self.inner
            .resource_unstopped(app, kind, name, Some(Arc::clone(&self.actor)));
    }

    // r[impl actuate.volume.hold.events]
    pub fn held_volume_created(
        &self,
        held_id: HeldVolumeId,
        app: &AppName,
        volume_name: &str,
        reason: &str,
    ) {
        self.inner.held_volume_created(
            held_id,
            app,
            volume_name,
            reason,
            Some(Arc::clone(&self.actor)),
        );
    }

    // r[impl actuate.volume.hold.events]
    pub fn held_volume_deleted(&self, held_id: HeldVolumeId) {
        self.inner
            .held_volume_deleted(held_id, Some(Arc::clone(&self.actor)));
    }

    // r[impl actuate.volume.hold.events]
    pub fn held_volume_restored(&self, held_id: HeldVolumeId, site_name: &str) {
        self.inner
            .held_volume_restored(held_id, site_name, Some(Arc::clone(&self.actor)));
    }

    // r[impl volume.site.lifecycle.events]
    pub fn site_volume_created(&self, name: &str, kind: &str, host_path: Option<&str>) {
        self.inner
            .site_volume_created(name, kind, host_path, Some(Arc::clone(&self.actor)));
    }

    // r[impl volume.site.lifecycle.events]
    pub fn site_volume_deleted(&self, name: &str, kind: &str, held_id: Option<HeldVolumeId>) {
        self.inner
            .site_volume_deleted(name, kind, held_id, Some(Arc::clone(&self.actor)));
    }

    // r[impl volume.site.snapshot.events]
    pub fn site_volume_snapshotted(&self, name: &str, source: &VolumeRef) {
        self.inner
            .site_volume_snapshotted(name, source, Some(Arc::clone(&self.actor)));
    }

    // r[impl volume.site.promote.events]
    pub fn site_volume_promoted(&self, name: &str, source: &str) {
        self.inner
            .site_volume_promoted(name, source, Some(Arc::clone(&self.actor)));
    }

    // r[impl volume.external.mapping.events]
    pub fn external_volume_mapped(
        &self,
        app: &AppName,
        external_name: &ExternalVolumeName,
        target: &VolumeRef,
        read_only: bool,
    ) {
        self.inner.external_volume_mapped(
            app,
            external_name,
            target,
            read_only,
            Some(Arc::clone(&self.actor)),
        );
    }

    // r[impl volume.external.mapping.events]
    pub fn external_volume_unmapped(&self, app: &AppName, external_name: &ExternalVolumeName) {
        self.inner
            .external_volume_unmapped(app, external_name, Some(Arc::clone(&self.actor)));
    }

    // r[impl volume.external.mapping.events]
    pub fn external_volume_remapped(
        &self,
        app: &AppName,
        external_name: &ExternalVolumeName,
        new: ExternalMappingSnapshot<'_>,
        previous: ExternalMappingSnapshot<'_>,
    ) {
        self.inner.external_volume_remapped(
            app,
            external_name,
            new,
            previous,
            Some(Arc::clone(&self.actor)),
        );
    }

    // r[impl service.site.lifecycle.events]
    pub fn site_service_created(&self, name: &str, description: Option<&str>) {
        self.inner
            .site_service_created(name, description, Some(Arc::clone(&self.actor)));
    }

    // r[impl service.site.lifecycle.events]
    pub fn site_service_deleted(&self, name: &str) {
        self.inner
            .site_service_deleted(name, Some(Arc::clone(&self.actor)));
    }

    // r[impl service.site.lifecycle.events]
    pub fn site_service_endpoint_added(
        &self,
        name: &str,
        service_port: u16,
        protocol: &str,
        remote_host: &str,
        remote_port: u16,
    ) {
        self.inner.site_service_endpoint_added(
            name,
            service_port,
            protocol,
            remote_host,
            remote_port,
            Some(Arc::clone(&self.actor)),
        );
    }

    // r[impl service.site.lifecycle.events]
    pub fn site_service_endpoint_removed(
        &self,
        name: &str,
        service_port: u16,
        protocol: &str,
        remote_host: &str,
        remote_port: u16,
    ) {
        self.inner.site_service_endpoint_removed(
            name,
            service_port,
            protocol,
            remote_host,
            remote_port,
            Some(Arc::clone(&self.actor)),
        );
    }

    // r[impl service.external.mapping.events]
    pub fn external_service_mapped(
        &self,
        app: &AppName,
        external_name: &ExternalServiceName,
        target: &ServiceRef,
    ) {
        self.inner.external_service_mapped(
            app,
            external_name,
            target,
            Some(Arc::clone(&self.actor)),
        );
    }

    // r[impl service.external.mapping.events]
    pub fn external_service_unmapped(&self, app: &AppName, external_name: &ExternalServiceName) {
        self.inner
            .external_service_unmapped(app, external_name, Some(Arc::clone(&self.actor)));
    }

    // r[impl service.external.mapping.events]
    pub fn external_service_remapped(
        &self,
        app: &AppName,
        external_name: &ExternalServiceName,
        new: ExternalServiceMappingSnapshot<'_>,
        previous: ExternalServiceMappingSnapshot<'_>,
    ) {
        self.inner.external_service_remapped(
            app,
            external_name,
            new,
            previous,
            Some(Arc::clone(&self.actor)),
        );
    }

    // r[impl ingress.site.lifecycle.events]
    #[allow(clippy::too_many_arguments)]
    pub fn site_ingress_created(
        &self,
        name: &SiteIngressName,
        hostname: &str,
        source: &str,
        discovered_provider: Option<&str>,
        tls_provider: &str,
        description: Option<&str>,
    ) {
        self.inner.site_ingress_created(
            name,
            hostname,
            source,
            discovered_provider,
            tls_provider,
            description,
            Some(Arc::clone(&self.actor)),
        );
    }

    // r[impl ingress.site.lifecycle.events]
    pub fn site_ingress_updated(
        &self,
        name: &SiteIngressName,
        hostname: &str,
        tls_provider: &str,
        description: Option<&str>,
    ) {
        self.inner.site_ingress_updated(
            name,
            hostname,
            tls_provider,
            description,
            Some(Arc::clone(&self.actor)),
        );
    }

    // r[impl ingress.site.lifecycle.events]
    pub fn site_ingress_deleted(&self, name: &SiteIngressName, source: &str) {
        self.inner
            .site_ingress_deleted(name, source, Some(Arc::clone(&self.actor)));
    }

    // r[impl ingress.site.lifecycle.events]
    #[allow(clippy::too_many_arguments)]
    pub fn site_ingress_attachment_added(
        &self,
        name: &SiteIngressName,
        port: u16,
        protocol: &str,
        target_kind: &str,
        target_app: Option<&AppName>,
        target_service: Option<&AppServiceName>,
        redirect_url: Option<&str>,
        redirect_code: Option<u16>,
    ) {
        self.inner.site_ingress_attachment_added(
            name,
            port,
            protocol,
            target_kind,
            target_app,
            target_service,
            redirect_url,
            redirect_code,
            Some(Arc::clone(&self.actor)),
        );
    }

    // r[impl ingress.site.lifecycle.events]
    #[allow(clippy::too_many_arguments)]
    pub fn site_ingress_attachment_updated(
        &self,
        name: &SiteIngressName,
        port: u16,
        protocol: &str,
        target_kind: &str,
        target_app: Option<&AppName>,
        target_service: Option<&AppServiceName>,
        redirect_url: Option<&str>,
        redirect_code: Option<u16>,
    ) {
        self.inner.site_ingress_attachment_updated(
            name,
            port,
            protocol,
            target_kind,
            target_app,
            target_service,
            redirect_url,
            redirect_code,
            Some(Arc::clone(&self.actor)),
        );
    }

    // r[impl ingress.site.lifecycle.events]
    pub fn site_ingress_attachment_removed(
        &self,
        name: &SiteIngressName,
        port: u16,
        protocol: &str,
    ) {
        self.inner.site_ingress_attachment_removed(
            name,
            port,
            protocol,
            Some(Arc::clone(&self.actor)),
        );
    }

    // r[impl audit.log.events]
    pub fn template_created(&self, name: &TemplateName) {
        self.inner
            .template_created(name, Some(Arc::clone(&self.actor)));
    }

    // r[impl audit.log.events]
    pub fn template_updated(&self, name: &TemplateName) {
        self.inner
            .template_updated(name, Some(Arc::clone(&self.actor)));
    }

    // r[impl audit.log.events]
    pub fn template_removed(&self, name: &TemplateName) {
        self.inner
            .template_removed(name, Some(Arc::clone(&self.actor)));
    }

    // r[impl audit.log.events]
    pub fn template_instantiated(&self, template: &TemplateName, app: &AppName) {
        self.inner
            .template_instantiated(template, app, Some(Arc::clone(&self.actor)));
    }
}

/// Context for operation lifecycle events (started / completed / failed).
/// Carries common fields so each call site only supplies what differs.
#[derive(Clone)]
pub struct OperationEventCtx {
    tx: EventSender,
    pub app: AppName,
    pub action_name: ActionName,
    pub operation_id: String,
    pub source_generation: u64,
    pub target_generation: u64,
    actor: Option<Arc<Actor>>,
}

impl OperationEventCtx {
    pub fn started(&self, trigger: &str) {
        self.tx.emit(OiEvent::OperationStarted {
            timestamp: now(),
            app: self.app.clone(),
            action_name: self.action_name.clone(),
            operation_id: self.operation_id.clone(),
            source_generation: self.source_generation,
            target_generation: self.target_generation,
            trigger: trigger.to_owned(),
            actor: self.actor.clone(),
        });
    }

    pub fn completed(&self) {
        self.tx.emit(OiEvent::OperationCompleted {
            timestamp: now(),
            app: self.app.clone(),
            action_name: self.action_name.clone(),
            operation_id: self.operation_id.clone(),
            source_generation: self.source_generation,
            target_generation: self.target_generation,
            actor: self.actor.clone(),
        });
    }

    pub fn failed(&self, error: &str) {
        self.tx.emit(OiEvent::OperationFailed {
            timestamp: now(),
            app: self.app.clone(),
            action_name: self.action_name.clone(),
            operation_id: self.operation_id.clone(),
            source_generation: self.source_generation,
            target_generation: self.target_generation,
            error: error.to_owned(),
            actor: self.actor.clone(),
        });
    }
}

/// Context for parameter-change events (set / unset).
#[derive(Clone)]
pub struct ParamEventCtx {
    tx: EventSender,
    app: AppName,
    generation: u64,
    previous_generation: u64,
    actor: Option<Arc<Actor>>,
}

impl ParamEventCtx {
    pub fn set(&self, name: &ParamName, previous_value: Option<&str>, new_value: &str) {
        self.tx.emit(OiEvent::ParamSet {
            timestamp: now(),
            app: self.app.clone(),
            name: name.clone(),
            previous_value: previous_value.map(str::to_owned),
            new_value: Some(new_value.to_owned()),
            redacted: false,
            generation: self.generation,
            previous_generation: self.previous_generation,
            actor: self.actor.clone(),
        });
    }

    // i[impl param.store.secret]
    pub fn set_redacted(&self, name: &ParamName) {
        self.tx.emit(OiEvent::ParamSet {
            timestamp: now(),
            app: self.app.clone(),
            name: name.clone(),
            previous_value: None,
            new_value: None,
            redacted: true,
            generation: self.generation,
            previous_generation: self.previous_generation,
            actor: self.actor.clone(),
        });
    }

    pub fn unset(&self, name: &ParamName, previous_value: &str) {
        self.tx.emit(OiEvent::ParamUnset {
            timestamp: now(),
            app: self.app.clone(),
            name: name.clone(),
            previous_value: Some(previous_value.to_owned()),
            redacted: false,
            generation: self.generation,
            previous_generation: self.previous_generation,
            actor: self.actor.clone(),
        });
    }

    // i[impl param.store.secret]
    pub fn unset_redacted(&self, name: &ParamName) {
        self.tx.emit(OiEvent::ParamUnset {
            timestamp: now(),
            app: self.app.clone(),
            name: name.clone(),
            previous_value: None,
            redacted: true,
            generation: self.generation,
            previous_generation: self.previous_generation,
            actor: self.actor.clone(),
        });
    }
}

/// Context for scale-change events.
/// Captures deployment identity and bounds; call `.changed(new, prev)` to emit.
#[derive(Clone)]
pub struct ScaleEventCtx {
    tx: EventSender,
    app: AppName,
    deployment: String,
    bounds_low: u16,
    bounds_high: u16,
    actor: Option<Arc<Actor>>,
}

impl ScaleEventCtx {
    pub fn changed(&self, scale: u16, previous_scale: u16) {
        self.tx.emit(OiEvent::ScaleChanged {
            timestamp: now(),
            app: self.app.clone(),
            deployment: self.deployment.clone(),
            scale,
            previous_scale,
            bounds_low: self.bounds_low,
            bounds_high: self.bounds_high,
            actor: self.actor.clone(),
        });
    }
}

#[cfg(test)]
mod tests;
