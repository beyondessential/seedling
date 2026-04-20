use std::sync::Arc;

use jiff::Timestamp;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::actor::Actor;

// i[event.types]
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum OiEvent {
    // r[impl audit.log.generations]
    AppRegistered {
        timestamp: Timestamp,
        app: String,
        generation: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    AppDeregistered {
        timestamp: Timestamp,
        app: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl audit.log.generations]
    AppUpdated {
        timestamp: Timestamp,
        app: String,
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
        app: String,
        phase: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl audit.log.generations]
    // i[impl param.store.secret]
    ParamSet {
        timestamp: Timestamp,
        app: String,
        name: String,
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
        app: String,
        name: String,
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
        app: String,
        action_name: String,
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
        app: String,
        action_name: String,
        operation_id: String,
        source_generation: u64,
        target_generation: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl operation.lifecycle.generations]
    OperationFailed {
        timestamp: Timestamp,
        app: String,
        action_name: String,
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
        app: String,
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
        app: String,
        /// Kind of the fault that was cleared. Mirrors
        /// [`FaultFiled::kind`] so UIs can render a useful summary without
        /// having to remember every fault ID they saw.
        kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    ResourceStateChanged {
        timestamp: Timestamp,
        app: String,
        resource_type: String,
        resource_name: String,
        instance_id: String,
        state: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    ShellStarted {
        timestamp: Timestamp,
        session_id: String,
        app: String,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    ShellExited {
        timestamp: Timestamp,
        session_id: String,
        exit_code: i32,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    ForwardStarted {
        timestamp: Timestamp,
        forward_id: String,
        app: String,
        service: String,
        port: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    ForwardStopped {
        timestamp: Timestamp,
        forward_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    ScaleChanged {
        timestamp: Timestamp,
        app: String,
        deployment: String,
        scale: u16,
        previous_scale: u16,
        bounds_low: u16,
        bounds_high: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl deployment.restart]
    DeploymentRestarted {
        timestamp: Timestamp,
        app: String,
        deployment: String,
        operation_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl resource.stop]
    ResourceStopped {
        timestamp: Timestamp,
        app: String,
        kind: String,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        actor: Option<Arc<Actor>>,
    },
    // r[impl resource.unstop]
    ResourceUnstopped {
        timestamp: Timestamp,
        app: String,
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

impl EventSender {
    fn emit(&self, event: OiEvent) {
        let _ = self.0.send(event);
    }

    pub fn app_registered(&self, app: &str, generation: u64, actor: Option<Arc<Actor>>) {
        self.emit(OiEvent::AppRegistered {
            timestamp: now(),
            app: app.to_owned(),
            generation,
            actor,
        });
    }

    pub fn app_deregistered(&self, app: &str, actor: Option<Arc<Actor>>) {
        self.emit(OiEvent::AppDeregistered {
            timestamp: now(),
            app: app.to_owned(),
            actor,
        });
    }

    pub fn app_updated(
        &self,
        app: &str,
        generation: u64,
        previous_generation: Option<u64>,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::AppUpdated {
            timestamp: now(),
            app: app.to_owned(),
            generation,
            previous_generation,
            actor,
        });
    }

    /// Emit `AppPhaseChanged`. `phase` is the lowercase snake-case phase name
    /// matching [`crate::events::OiEvent::AppPhaseChanged::phase`].
    pub fn app_phase_changed(&self, app: &str, phase: &str, actor: Option<Arc<Actor>>) {
        self.emit(OiEvent::AppPhaseChanged {
            timestamp: now(),
            app: app.to_owned(),
            phase: phase.to_owned(),
            actor,
        });
    }

    /// Build a context for scale-change events.
    /// Captures the deployment identity and bounds; call `.changed(new, prev)` to emit.
    pub fn scale(
        &self,
        app: impl Into<String>,
        deployment: impl Into<String>,
        bounds_low: u16,
        bounds_high: u16,
        actor: Option<Arc<Actor>>,
    ) -> ScaleEventCtx {
        ScaleEventCtx {
            tx: self.clone(),
            app: app.into(),
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
        app: &str,
        resource_type: Option<&str>,
        resource_name: Option<&str>,
        instance_id: Option<&str>,
        kind: &str,
        description: &str,
    ) {
        self.emit(OiEvent::FaultFiled {
            timestamp: now(),
            id: id.to_owned(),
            app: app.to_owned(),
            resource_type: resource_type.map(str::to_owned),
            resource_name: resource_name.map(str::to_owned),
            instance_id: instance_id.map(str::to_owned),
            kind: kind.to_owned(),
            description: description.to_owned(),
            actor: None,
        });
    }

    pub fn fault_cleared(&self, id: &str, app: &str, kind: &str) {
        self.emit(OiEvent::FaultCleared {
            timestamp: now(),
            id: id.to_owned(),
            app: app.to_owned(),
            kind: kind.to_owned(),
            actor: None,
        });
    }

    pub fn resource_state_changed(
        &self,
        app: &str,
        resource_type: &str,
        resource_name: &str,
        instance_id: &str,
        state: &str,
    ) {
        self.emit(OiEvent::ResourceStateChanged {
            timestamp: now(),
            app: app.to_owned(),
            resource_type: resource_type.to_owned(),
            resource_name: resource_name.to_owned(),
            instance_id: instance_id.to_owned(),
            state: state.to_owned(),
            actor: None,
        });
    }

    // i[impl shell.start]
    pub fn shell_started(&self, session_id: &str, app: &str, name: &str) {
        self.emit(OiEvent::ShellStarted {
            timestamp: now(),
            session_id: session_id.to_owned(),
            app: app.to_owned(),
            name: name.to_owned(),
            actor: None,
        });
    }

    // i[impl shell.exit]
    pub fn shell_exited(&self, session_id: &str, exit_code: i32) {
        self.emit(OiEvent::ShellExited {
            timestamp: now(),
            session_id: session_id.to_owned(),
            exit_code,
            actor: None,
        });
    }

    // i[impl forward.start]
    pub fn forward_started(&self, forward_id: &str, app: &str, service: &str, port: u16) {
        self.emit(OiEvent::ForwardStarted {
            timestamp: now(),
            forward_id: forward_id.to_owned(),
            app: app.to_owned(),
            service: service.to_owned(),
            port,
            actor: None,
        });
    }

    // i[impl forward.start]
    pub fn forward_stopped(&self, forward_id: &str) {
        self.emit(OiEvent::ForwardStopped {
            timestamp: now(),
            forward_id: forward_id.to_owned(),
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

    // r[impl deployment.restart]
    pub fn deployment_restarted(
        &self,
        app: &str,
        deployment: &str,
        operation_id: &str,
        actor: Option<Arc<Actor>>,
    ) {
        self.emit(OiEvent::DeploymentRestarted {
            timestamp: now(),
            app: app.to_owned(),
            deployment: deployment.to_owned(),
            operation_id: operation_id.to_owned(),
            actor,
        });
    }

    // r[impl resource.stop]
    pub fn resource_stopped(&self, app: &str, kind: &str, name: &str, actor: Option<Arc<Actor>>) {
        self.emit(OiEvent::ResourceStopped {
            timestamp: now(),
            app: app.to_owned(),
            kind: kind.to_owned(),
            name: name.to_owned(),
            actor,
        });
    }

    // r[impl resource.unstop]
    pub fn resource_unstopped(&self, app: &str, kind: &str, name: &str, actor: Option<Arc<Actor>>) {
        self.emit(OiEvent::ResourceUnstopped {
            timestamp: now(),
            app: app.to_owned(),
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
        app: impl Into<String>,
        action_name: impl Into<String>,
        operation_id: impl Into<String>,
        source_generation: u64,
        target_generation: u64,
        actor: Option<Arc<Actor>>,
    ) -> OperationEventCtx {
        OperationEventCtx {
            tx: self.clone(),
            app: app.into(),
            action_name: action_name.into(),
            operation_id: operation_id.into(),
            source_generation,
            target_generation,
            actor,
        }
    }

    /// Build a context for param-change events (set/unset).
    pub fn param_change(
        &self,
        app: impl Into<String>,
        generation: u64,
        previous_generation: u64,
        actor: Option<Arc<Actor>>,
    ) -> ParamEventCtx {
        ParamEventCtx {
            tx: self.clone(),
            app: app.into(),
            generation,
            previous_generation,
            actor,
        }
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

    pub fn app_registered(&self, app: &str, generation: u64) {
        self.inner
            .app_registered(app, generation, Some(Arc::clone(&self.actor)));
    }

    pub fn app_deregistered(&self, app: &str) {
        self.inner
            .app_deregistered(app, Some(Arc::clone(&self.actor)));
    }

    pub fn app_updated(&self, app: &str, generation: u64, previous_generation: Option<u64>) {
        self.inner.app_updated(
            app,
            generation,
            previous_generation,
            Some(Arc::clone(&self.actor)),
        );
    }

    pub fn app_phase_changed(&self, app: &str, phase: &str) {
        self.inner
            .app_phase_changed(app, phase, Some(Arc::clone(&self.actor)));
    }

    pub fn scale(
        &self,
        app: impl Into<String>,
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
        app: impl Into<String>,
        action_name: impl Into<String>,
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
        app: impl Into<String>,
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

    // r[impl deployment.restart]
    pub fn deployment_restarted(&self, app: &str, deployment: &str, operation_id: &str) {
        self.inner.deployment_restarted(
            app,
            deployment,
            operation_id,
            Some(Arc::clone(&self.actor)),
        );
    }

    // r[impl resource.stop]
    pub fn resource_stopped(&self, app: &str, kind: &str, name: &str) {
        self.inner
            .resource_stopped(app, kind, name, Some(Arc::clone(&self.actor)));
    }

    // r[impl resource.unstop]
    pub fn resource_unstopped(&self, app: &str, kind: &str, name: &str) {
        self.inner
            .resource_unstopped(app, kind, name, Some(Arc::clone(&self.actor)));
    }
}

/// Context for operation lifecycle events (started / completed / failed).
/// Carries common fields so each call site only supplies what differs.
#[derive(Clone)]
pub struct OperationEventCtx {
    tx: EventSender,
    pub app: String,
    pub action_name: String,
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
    app: String,
    generation: u64,
    previous_generation: u64,
    actor: Option<Arc<Actor>>,
}

impl ParamEventCtx {
    pub fn set(&self, name: &str, previous_value: Option<&str>, new_value: &str) {
        self.tx.emit(OiEvent::ParamSet {
            timestamp: now(),
            app: self.app.clone(),
            name: name.to_owned(),
            previous_value: previous_value.map(str::to_owned),
            new_value: Some(new_value.to_owned()),
            redacted: false,
            generation: self.generation,
            previous_generation: self.previous_generation,
            actor: self.actor.clone(),
        });
    }

    // i[impl param.store.secret]
    pub fn set_redacted(&self, name: &str) {
        self.tx.emit(OiEvent::ParamSet {
            timestamp: now(),
            app: self.app.clone(),
            name: name.to_owned(),
            previous_value: None,
            new_value: None,
            redacted: true,
            generation: self.generation,
            previous_generation: self.previous_generation,
            actor: self.actor.clone(),
        });
    }

    pub fn unset(&self, name: &str, previous_value: &str) {
        self.tx.emit(OiEvent::ParamUnset {
            timestamp: now(),
            app: self.app.clone(),
            name: name.to_owned(),
            previous_value: Some(previous_value.to_owned()),
            redacted: false,
            generation: self.generation,
            previous_generation: self.previous_generation,
            actor: self.actor.clone(),
        });
    }

    // i[impl param.store.secret]
    pub fn unset_redacted(&self, name: &str) {
        self.tx.emit(OiEvent::ParamUnset {
            timestamp: now(),
            app: self.app.clone(),
            name: name.to_owned(),
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
    app: String,
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
