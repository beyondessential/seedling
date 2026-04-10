use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::broadcast;

// i[event.types]
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum OiEvent {
    AppRegistered {
        timestamp: DateTime<Utc>,
        app: String,
    },
    AppDeregistered {
        timestamp: DateTime<Utc>,
        app: String,
    },
    AppUpdated {
        timestamp: DateTime<Utc>,
        app: String,
    },
    OperationStarted {
        timestamp: DateTime<Utc>,
        app: String,
        action_name: String,
        operation_id: String,
    },
    OperationCompleted {
        timestamp: DateTime<Utc>,
        app: String,
        action_name: String,
        operation_id: String,
    },
    OperationFailed {
        timestamp: DateTime<Utc>,
        app: String,
        action_name: String,
        operation_id: String,
        error: String,
    },
    FaultFiled {
        timestamp: DateTime<Utc>,
        id: String,
        app: String,
        resource_type: Option<String>,
        resource_name: Option<String>,
        instance_id: Option<String>,
        kind: String,
        description: String,
    },
    FaultCleared {
        timestamp: DateTime<Utc>,
        id: String,
        app: String,
    },
    ResourceStateChanged {
        timestamp: DateTime<Utc>,
        app: String,
        resource_type: String,
        resource_name: String,
        instance_id: String,
        state: String,
    },
    ShellExited {
        timestamp: DateTime<Utc>,
        session_id: String,
        exit_code: i32,
    },
    ForwardStarted {
        timestamp: DateTime<Utc>,
        forward_id: String,
        app: String,
        service: String,
        port: u16,
    },
    ForwardStopped {
        timestamp: DateTime<Utc>,
        forward_id: String,
    },
}

pub type EventSender = broadcast::Sender<OiEvent>;

pub fn new_event_channel() -> EventSender {
    let (tx, _) = broadcast::channel(256);
    tx
}

fn now() -> DateTime<Utc> {
    std::time::SystemTime::now().into()
}

/// Emit an event, ignoring the result (no subscribers is fine).
pub fn emit(tx: &EventSender, event: OiEvent) {
    let _ = tx.send(event);
}

pub fn app_registered(tx: &EventSender, app: &str) {
    emit(
        tx,
        OiEvent::AppRegistered {
            timestamp: now(),
            app: app.to_owned(),
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

pub fn app_updated(tx: &EventSender, app: &str) {
    emit(
        tx,
        OiEvent::AppUpdated {
            timestamp: now(),
            app: app.to_owned(),
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
