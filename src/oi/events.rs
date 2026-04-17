use jiff::Timestamp;
use serde::Serialize;
use tokio::sync::broadcast;

// i[event.types]
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum OiEvent {
    // r[impl audit.log.generations]
    AppRegistered {
        timestamp: Timestamp,
        app: String,
        generation: u64,
    },
    AppDeregistered {
        timestamp: Timestamp,
        app: String,
    },
    // r[impl audit.log.generations]
    AppUpdated {
        timestamp: Timestamp,
        app: String,
        generation: u64,
        previous_generation: Option<u64>,
    },
    // r[impl audit.log.generations]
    ParamSet {
        timestamp: Timestamp,
        app: String,
        name: String,
        previous_value: Option<String>,
        new_value: String,
        generation: u64,
        previous_generation: u64,
    },
    // r[impl audit.log.generations]
    ParamUnset {
        timestamp: Timestamp,
        app: String,
        name: String,
        previous_value: String,
        generation: u64,
        previous_generation: u64,
    },
    OperationStarted {
        timestamp: Timestamp,
        app: String,
        action_name: String,
        operation_id: String,
    },
    OperationCompleted {
        timestamp: Timestamp,
        app: String,
        action_name: String,
        operation_id: String,
    },
    OperationFailed {
        timestamp: Timestamp,
        app: String,
        action_name: String,
        operation_id: String,
        error: String,
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
    },
    FaultCleared {
        timestamp: Timestamp,
        id: String,
        app: String,
    },
    ResourceStateChanged {
        timestamp: Timestamp,
        app: String,
        resource_type: String,
        resource_name: String,
        instance_id: String,
        state: String,
    },
    ShellExited {
        timestamp: Timestamp,
        session_id: String,
        exit_code: i32,
    },
    ForwardStarted {
        timestamp: Timestamp,
        forward_id: String,
        app: String,
        service: String,
        port: u16,
    },
    ForwardStopped {
        timestamp: Timestamp,
        forward_id: String,
    },
    ScaleChanged {
        timestamp: Timestamp,
        app: String,
        deployment: String,
        scale: u16,
        previous_scale: u16,
        bounds_low: u16,
        bounds_high: u16,
    },
    ServerBusy {
        timestamp: Timestamp,
        reason: String,
    },
}

pub type EventSender = broadcast::Sender<OiEvent>;

pub fn new_event_channel() -> EventSender {
    let (tx, _) = broadcast::channel(256);
    tx
}

fn now() -> Timestamp {
    Timestamp::now()
}

/// Emit an event, ignoring the result (no subscribers is fine).
pub fn emit(tx: &EventSender, event: OiEvent) {
    let _ = tx.send(event);
}

pub fn app_registered(tx: &EventSender, app: &str, generation: u64) {
    emit(
        tx,
        OiEvent::AppRegistered {
            timestamp: now(),
            app: app.to_owned(),
            generation,
        },
    );
}

pub fn app_deregistered(tx: &EventSender, app: &str) {
    emit(
        tx,
        OiEvent::AppDeregistered {
            timestamp: now(),
            app: app.to_owned(),
        },
    );
}

pub fn app_updated(tx: &EventSender, app: &str, generation: u64, previous_generation: Option<u64>) {
    emit(
        tx,
        OiEvent::AppUpdated {
            timestamp: now(),
            app: app.to_owned(),
            generation,
            previous_generation,
        },
    );
}

pub fn param_set(
    tx: &EventSender,
    app: &str,
    name: &str,
    previous_value: Option<&str>,
    new_value: &str,
    generation: u64,
    previous_generation: u64,
) {
    emit(
        tx,
        OiEvent::ParamSet {
            timestamp: now(),
            app: app.to_owned(),
            name: name.to_owned(),
            previous_value: previous_value.map(str::to_owned),
            new_value: new_value.to_owned(),
            generation,
            previous_generation,
        },
    );
}

pub fn param_unset(
    tx: &EventSender,
    app: &str,
    name: &str,
    previous_value: &str,
    generation: u64,
    previous_generation: u64,
) {
    emit(
        tx,
        OiEvent::ParamUnset {
            timestamp: now(),
            app: app.to_owned(),
            name: name.to_owned(),
            previous_value: previous_value.to_owned(),
            generation,
            previous_generation,
        },
    );
}

pub fn operation_started(tx: &EventSender, app: &str, action_name: &str, operation_id: &str) {
    emit(
        tx,
        OiEvent::OperationStarted {
            timestamp: now(),
            app: app.to_owned(),
            action_name: action_name.to_owned(),
            operation_id: operation_id.to_owned(),
        },
    );
}

pub fn operation_completed(tx: &EventSender, app: &str, action_name: &str, operation_id: &str) {
    emit(
        tx,
        OiEvent::OperationCompleted {
            timestamp: now(),
            app: app.to_owned(),
            action_name: action_name.to_owned(),
            operation_id: operation_id.to_owned(),
        },
    );
}

pub fn operation_failed(
    tx: &EventSender,
    app: &str,
    action_name: &str,
    operation_id: &str,
    error: &str,
) {
    emit(
        tx,
        OiEvent::OperationFailed {
            timestamp: now(),
            app: app.to_owned(),
            action_name: action_name.to_owned(),
            operation_id: operation_id.to_owned(),
            error: error.to_owned(),
        },
    );
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors all fields of OiEvent::FaultFiled"
)]
pub fn fault_filed(
    tx: &EventSender,
    id: &str,
    app: &str,
    resource_type: Option<&str>,
    resource_name: Option<&str>,
    instance_id: Option<&str>,
    kind: &str,
    description: &str,
) {
    emit(
        tx,
        OiEvent::FaultFiled {
            timestamp: now(),
            id: id.to_owned(),
            app: app.to_owned(),
            resource_type: resource_type.map(str::to_owned),
            resource_name: resource_name.map(str::to_owned),
            instance_id: instance_id.map(str::to_owned),
            kind: kind.to_owned(),
            description: description.to_owned(),
        },
    );
}

pub fn fault_cleared(tx: &EventSender, id: &str, app: &str) {
    emit(
        tx,
        OiEvent::FaultCleared {
            timestamp: now(),
            id: id.to_owned(),
            app: app.to_owned(),
        },
    );
}

pub fn resource_state_changed(
    tx: &EventSender,
    app: &str,
    resource_type: &str,
    resource_name: &str,
    instance_id: &str,
    state: &str,
) {
    emit(
        tx,
        OiEvent::ResourceStateChanged {
            timestamp: now(),
            app: app.to_owned(),
            resource_type: resource_type.to_owned(),
            resource_name: resource_name.to_owned(),
            instance_id: instance_id.to_owned(),
            state: state.to_owned(),
        },
    );
}

pub fn shell_exited(tx: &EventSender, session_id: &str, exit_code: i32) {
    emit(
        tx,
        OiEvent::ShellExited {
            timestamp: now(),
            session_id: session_id.to_owned(),
            exit_code,
        },
    );
}

pub fn forward_started(tx: &EventSender, forward_id: &str, app: &str, service: &str, port: u16) {
    emit(
        tx,
        OiEvent::ForwardStarted {
            timestamp: now(),
            forward_id: forward_id.to_owned(),
            app: app.to_owned(),
            service: service.to_owned(),
            port,
        },
    );
}

pub fn forward_stopped(tx: &EventSender, forward_id: &str) {
    emit(
        tx,
        OiEvent::ForwardStopped {
            timestamp: now(),
            forward_id: forward_id.to_owned(),
        },
    );
}

pub fn scale_changed(
    tx: &EventSender,
    app: &str,
    deployment: &str,
    scale: u16,
    previous_scale: u16,
    bounds_low: u16,
    bounds_high: u16,
) {
    emit(
        tx,
        OiEvent::ScaleChanged {
            timestamp: now(),
            app: app.to_owned(),
            deployment: deployment.to_owned(),
            scale,
            previous_scale,
            bounds_low,
            bounds_high,
        },
    );
}

pub fn server_busy(tx: &EventSender, reason: &str) {
    emit(
        tx,
        OiEvent::ServerBusy {
            timestamp: now(),
            reason: reason.to_owned(),
        },
    );
}
