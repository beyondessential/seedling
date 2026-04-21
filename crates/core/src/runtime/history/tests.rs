use seedling_protocol::names::AppName;

use super::*;
use crate::defs::resource::ResourceKind;
use crate::runtime::barrier::{ActionLogEntry, BarrierRecord, CallKind, OperationId};
use crate::runtime::db::Db;
use crate::runtime::identity::{InstanceId, InstanceVariant, ResourceInstance};
use crate::runtime::lifecycle::LifecycleState;

fn app_name(s: &str) -> AppName {
    AppName::new(s).unwrap()
}

fn dep(app: &str, name: &str) -> ResourceInstance {
    ResourceInstance::new_singleton(app_name(app), ResourceKind::Deployment, name)
}

// -----------------------------------------------------------------------
// Current operation
// -----------------------------------------------------------------------

// r[verify history.persistence]
#[test]
fn save_and_load_current_operation() {
    use crate::runtime::secrets::Cipher;

    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let op = CurrentOperation {
        operation_id: OperationId("test-op-id".into()),
        app: app_name("myapp"),
        action_name: "start".into(),
        source_generation: 3,
        target_generation: 4,
    };
    save_current_operation(&db, &cipher, &op, &serde_json::Map::new()).unwrap();
    let loaded = load_current_operation(&db).unwrap().unwrap();
    assert_eq!(loaded.operation_id.0, "test-op-id");
    assert_eq!(loaded.app, "myapp");
    assert_eq!(loaded.action_name, "start");
    assert_eq!(loaded.source_generation, 3);
    assert_eq!(loaded.target_generation, 4);
}

#[test]
fn load_current_operation_returns_none_when_empty() {
    let db = Db::open_in_memory().unwrap();
    assert!(load_current_operation(&db).unwrap().is_none());
}

#[test]
fn clear_current_operation_removes_record() {
    use crate::runtime::secrets::Cipher;

    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let op = CurrentOperation {
        operation_id: OperationId("op-1".into()),
        app: app_name("app"),
        action_name: "start".into(),
        source_generation: 1,
        target_generation: 1,
    };
    save_current_operation(&db, &cipher, &op, &serde_json::Map::new()).unwrap();
    clear_current_operation(&db).unwrap();
    assert!(load_current_operation(&db).unwrap().is_none());
}

// r[verify operation.cancel.persistence]
#[test]
fn cancel_requested_round_trips_through_db() {
    use crate::runtime::secrets::Cipher;

    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let op = CurrentOperation {
        operation_id: OperationId("op-c1".into()),
        app: app_name("app"),
        action_name: "save-snapshot".into(),
        source_generation: 1,
        target_generation: 1,
    };
    save_current_operation(&db, &cipher, &op, &serde_json::Map::new()).unwrap();

    // Fresh rows default to cancel_requested = false.
    assert!(!load_cancel_requested(&db).unwrap());

    // Matching op_id: flag flips and persists.
    let flipped = set_cancel_requested(&db, &OperationId("op-c1".into())).unwrap();
    assert!(flipped);
    assert!(load_cancel_requested(&db).unwrap());

    // Non-matching op_id: no-op (a later op with a different id must not
    // inherit the cancel from a stale row).
    clear_current_operation(&db).unwrap();
    save_current_operation(&db, &cipher, &op, &serde_json::Map::new()).unwrap();
    let flipped = set_cancel_requested(&db, &OperationId("other-op".into())).unwrap();
    assert!(!flipped);
    assert!(!load_cancel_requested(&db).unwrap());

    // No current_operation row at all: load returns false; set returns false.
    clear_current_operation(&db).unwrap();
    assert!(!load_cancel_requested(&db).unwrap());
    let flipped = set_cancel_requested(&db, &OperationId("op-c1".into())).unwrap();
    assert!(!flipped);
}

// r[verify operation.params] i[verify action.invoke.install.validation]
#[test]
fn install_params_round_trip_through_cipher() {
    use crate::runtime::secrets::Cipher;

    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let op = CurrentOperation {
        operation_id: OperationId("install-op".into()),
        app: app_name("myapp"),
        action_name: "install".into(),
        source_generation: 1,
        target_generation: 1,
    };
    let mut params = serde_json::Map::new();
    params.insert("passphrase".into(), serde_json::json!("hunter2"));
    params.insert("bucket".into(), serde_json::json!("my-backups"));

    save_current_operation(&db, &cipher, &op, &params).unwrap();

    // Metadata loads via the normal path.
    let meta = load_current_operation(&db).unwrap().unwrap();
    assert_eq!(meta.action_name, "install");

    // Params decrypt back to the original map.
    let restored = load_current_operation_params(&db, &cipher)
        .unwrap()
        .unwrap();
    assert_eq!(restored, params);

    // Clearing the row clears the ciphertext too.
    clear_current_operation(&db).unwrap();
    assert!(
        load_current_operation_params(&db, &cipher)
            .unwrap()
            .is_none()
    );
}

// r[verify operation.params]
#[test]
fn non_install_operation_params_round_trip() {
    use crate::runtime::secrets::Cipher;

    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let op = CurrentOperation {
        operation_id: OperationId("invoke-op".into()),
        app: app_name("myapp"),
        action_name: "rotate_secret".into(),
        source_generation: 5,
        target_generation: 5,
    };
    // Operator-invoked actions may pass params that include secret-ish
    // values. On replay those params must be restored.
    let mut params = serde_json::Map::new();
    params.insert("target_host".into(), serde_json::json!("db-1.internal"));
    params.insert("auth_token".into(), serde_json::json!("s3cr3t"));

    save_current_operation(&db, &cipher, &op, &params).unwrap();

    let restored = load_current_operation_params(&db, &cipher)
        .unwrap()
        .unwrap();
    assert_eq!(restored, params);
}

// r[verify operation.params]
#[test]
fn empty_params_round_trip() {
    use crate::runtime::secrets::Cipher;

    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let op = CurrentOperation {
        operation_id: OperationId("start-op".into()),
        app: app_name("myapp"),
        action_name: "start".into(),
        source_generation: 1,
        target_generation: 1,
    };
    save_current_operation(&db, &cipher, &op, &serde_json::Map::new()).unwrap();

    // Empty params map round-trips through the cipher without losing
    // identity; replay restores it as an empty map, not as None.
    let restored = load_current_operation_params(&db, &cipher)
        .unwrap()
        .unwrap();
    assert!(restored.is_empty());
}

#[test]
fn save_overwrites_previous_current_operation() {
    use crate::runtime::secrets::Cipher;

    let db = Db::open_in_memory().unwrap();
    let cipher = Cipher::for_tests();
    let op1 = CurrentOperation {
        operation_id: OperationId("op-1".into()),
        app: app_name("app"),
        action_name: "start".into(),
        source_generation: 1,
        target_generation: 1,
    };
    let op2 = CurrentOperation {
        operation_id: OperationId("op-2".into()),
        app: app_name("app"),
        action_name: "stop".into(),
        source_generation: 1,
        target_generation: 1,
    };
    save_current_operation(&db, &cipher, &op1, &serde_json::Map::new()).unwrap();
    save_current_operation(&db, &cipher, &op2, &serde_json::Map::new()).unwrap();
    let loaded = load_current_operation(&db).unwrap().unwrap();
    assert_eq!(loaded.operation_id.0, "op-2");
    assert_eq!(loaded.action_name, "stop");
}

// -----------------------------------------------------------------------
// Instance registry
// -----------------------------------------------------------------------

// r[verify identity.stable]
// r[verify identity.components]
#[test]
fn insert_and_find_instance() {
    let db = Db::open_in_memory().unwrap();
    let instance = dep("myapp", "web");
    insert_instance(&db, &instance).unwrap();

    let found = find_instance(&db, instance.id).unwrap().unwrap();
    assert_eq!(found.id, instance.id);
    assert_eq!(found.app, "myapp");
    assert_eq!(found.name.as_deref(), Some("web"));
    assert_eq!(found.display_name, instance.display_name);
}

// r[verify identity.stable]
#[test]
fn find_instance_returns_none_for_unknown_id() {
    let db = Db::open_in_memory().unwrap();
    let id = InstanceId::generate();
    assert!(find_instance(&db, id).unwrap().is_none());
}

// r[verify identity.stable]
#[test]
fn insert_instance_is_idempotent() {
    let db = Db::open_in_memory().unwrap();
    let instance = dep("myapp", "web");
    insert_instance(&db, &instance).unwrap();
    insert_instance(&db, &instance).unwrap();
    let found = find_instance(&db, instance.id).unwrap().unwrap();
    assert_eq!(found.id, instance.id);
}

// r[verify identity.stable]
#[test]
fn get_or_create_singleton_creates_on_first_call() {
    let db = Db::open_in_memory().unwrap();
    let instance =
        get_or_create_singleton(&db, &app_name("myapp"), ResourceKind::Deployment, Some("web")).unwrap();
    assert_eq!(instance.app, "myapp");
    assert_eq!(instance.name.as_deref(), Some("web"));
    assert_eq!(instance.variant, InstanceVariant::Singleton);
}

// r[verify identity.stable]
#[test]
fn get_or_create_singleton_returns_same_id_on_second_call() {
    let db = Db::open_in_memory().unwrap();
    let a = get_or_create_singleton(&db, &app_name("myapp"), ResourceKind::Deployment, Some("web")).unwrap();
    let b = get_or_create_singleton(&db, &app_name("myapp"), ResourceKind::Deployment, Some("web")).unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(a.display_name, b.display_name);
}

// r[verify identity.components]
#[test]
fn find_instances_for_group_returns_all_scaled() {
    let db = Db::open_in_memory().unwrap();
    let a = ResourceInstance::new_scaled(app_name("myapp"), ResourceKind::Deployment, "web");
    let b = ResourceInstance::new_scaled(app_name("myapp"), ResourceKind::Deployment, "web");
    insert_instance(&db, &a).unwrap();
    insert_instance(&db, &b).unwrap();

    let found =
        find_instances_for_group(&db, &app_name("myapp"), ResourceKind::Deployment, Some("web"))
            .unwrap();
    assert_eq!(found.len(), 2);
    let ids: std::collections::HashSet<_> = found.iter().map(|i| i.id).collect();
    assert!(ids.contains(&a.id));
    assert!(ids.contains(&b.id));
}

// -----------------------------------------------------------------------
// World observations
// -----------------------------------------------------------------------

// r[verify history.world.entries]
#[test]
fn insert_and_retrieve_observation() {
    let db = Db::open_in_memory().unwrap();
    let resource = dep("app", "web");
    insert_observation(&db, &resource, "container_created", &serde_json::json!({})).unwrap();

    let obs = query_observations(&db, &resource).unwrap();
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].obs_kind, "container_created");
    assert_eq!(obs[0].resource.id, resource.id);
}

// r[verify history.world.entries]
#[test]
fn observations_ordered_by_recorded_at() {
    let db = Db::open_in_memory().unwrap();
    let resource = dep("app", "web");
    insert_observation(&db, &resource, "container_created", &serde_json::json!({})).unwrap();
    insert_observation(&db, &resource, "container_running", &serde_json::json!({})).unwrap();

    let obs = query_observations(&db, &resource).unwrap();
    assert_eq!(obs.len(), 2);
    assert_eq!(obs[0].obs_kind, "container_created");
    assert_eq!(obs[1].obs_kind, "container_running");
}

// r[verify history.world.entries]
#[test]
fn observations_scoped_to_instance_id() {
    let db = Db::open_in_memory().unwrap();
    let web = dep("app", "web");
    let api = dep("app", "api");
    insert_observation(&db, &web, "container_created", &serde_json::json!({})).unwrap();
    insert_observation(&db, &api, "container_running", &serde_json::json!({})).unwrap();

    let web_obs = query_observations(&db, &web).unwrap();
    assert_eq!(web_obs.len(), 1);
    assert_eq!(web_obs[0].obs_kind, "container_created");

    let api_obs = query_observations(&db, &api).unwrap();
    assert_eq!(api_obs.len(), 1);
    assert_eq!(api_obs[0].obs_kind, "container_running");
}

// r[verify history.world.entries]
#[test]
fn observations_empty_for_unknown_instance() {
    let db = Db::open_in_memory().unwrap();
    let resource = dep("app", "web");
    let obs = query_observations(&db, &resource).unwrap();
    assert!(obs.is_empty());
}

// -----------------------------------------------------------------------
// Autonomous operations
// -----------------------------------------------------------------------

// r[verify history.operations.entries]
#[test]
fn insert_and_retrieve_autonomous_operation() {
    let db = Db::open_in_memory().unwrap();
    let resource = dep("app", "web");
    let prov = Provenance {
        observation_ids: vec![1, 2],
        rule: "health-check-failed".into(),
    };
    let id = insert_autonomous_operation(&db, resource.id, "restart", &prov).unwrap();
    assert!(id > 0);

    let ops = query_autonomous_operations(&db, resource.id).unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].resource_id, resource.id);
    assert_eq!(ops[0].operation, "restart");
    assert!(ops[0].outcome.is_none());
}

// r[verify history.operations.entries]
#[test]
fn complete_autonomous_operation_sets_outcome() {
    let db = Db::open_in_memory().unwrap();
    let resource = dep("app", "web");
    let prov = Provenance {
        observation_ids: vec![],
        rule: "test".into(),
    };
    let id = insert_autonomous_operation(&db, resource.id, "restart", &prov).unwrap();
    complete_autonomous_operation(&db, id, "success").unwrap();

    let ops = query_autonomous_operations(&db, resource.id).unwrap();
    assert_eq!(ops[0].outcome.as_deref(), Some("success"));
    assert!(ops[0].completed_at.is_some());
}

// r[verify history.operations.entries]
#[test]
fn complete_autonomous_operation_with_error() {
    let db = Db::open_in_memory().unwrap();
    let resource = dep("app", "web");
    let prov = Provenance {
        observation_ids: vec![],
        rule: "test".into(),
    };
    let id = insert_autonomous_operation(&db, resource.id, "restart", &prov).unwrap();
    complete_autonomous_operation(&db, id, "error: container exited 1").unwrap();

    let ops = query_autonomous_operations(&db, resource.id).unwrap();
    assert_eq!(ops[0].outcome.as_deref(), Some("error: container exited 1"));
}

// r[verify history.operations.entries]
#[test]
fn autonomous_operations_scoped_to_instance_id() {
    let db = Db::open_in_memory().unwrap();
    let web = dep("app", "web");
    let api = dep("app", "api");
    let prov = Provenance {
        observation_ids: vec![],
        rule: "test".into(),
    };
    insert_autonomous_operation(&db, web.id, "restart", &prov).unwrap();
    insert_autonomous_operation(&db, api.id, "rebuild", &prov).unwrap();

    let web_ops = query_autonomous_operations(&db, web.id).unwrap();
    assert_eq!(web_ops.len(), 1);
    assert_eq!(web_ops[0].operation, "restart");

    let api_ops = query_autonomous_operations(&db, api.id).unwrap();
    assert_eq!(api_ops.len(), 1);
    assert_eq!(api_ops[0].operation, "rebuild");
}

// -----------------------------------------------------------------------
// Action log
// -----------------------------------------------------------------------

fn make_entry(
    call_index: usize,
    call_kind: CallKind,
    barrier: Option<BarrierRecord>,
) -> ActionLogEntry {
    ActionLogEntry {
        call_index,
        call_kind,
        resources: vec![dep("app", "web")],
        barrier,
    }
}

// r[verify history.action-log.entries]
#[test]
fn insert_and_load_action_log_entry_without_barrier() {
    let db = Db::open_in_memory().unwrap();
    let op = OperationId("op-1".into());
    let entry = make_entry(0, CallKind::Start, None);
    insert_action_log_entry(&db, &op, &app_name("myapp"), "start", &entry).unwrap();

    let loaded = load_action_log(&db, &op).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].call_index, 0);
    assert!(matches!(loaded[0].call_kind, CallKind::Start));
    assert!(loaded[0].barrier.is_none());
}

// r[verify history.action-log.entries]
#[test]
fn insert_and_load_action_log_entry_with_barrier() {
    let db = Db::open_in_memory().unwrap();
    let op = OperationId("op-1".into());
    let barrier = BarrierRecord {
        required_state: LifecycleState::Ready,
        deadline_secs: Some(30),
        satisfied: false,
        started_at_secs: Some(1000),
    };
    let entry = make_entry(0, CallKind::Start, Some(barrier));
    insert_action_log_entry(&db, &op, &app_name("myapp"), "start", &entry).unwrap();

    let loaded = load_action_log(&db, &op).unwrap();
    let b = loaded[0].barrier.as_ref().unwrap();
    assert_eq!(b.required_state, LifecycleState::Ready);
    assert_eq!(b.deadline_secs, Some(30));
    assert!(!b.satisfied);
    assert_eq!(b.started_at_secs, Some(1000));
}

// r[verify reconciliation.idempotency]
#[test]
fn barrier_satisfaction_update_via_replace() {
    let db = Db::open_in_memory().unwrap();
    let op = OperationId("op-1".into());
    let barrier = BarrierRecord {
        required_state: LifecycleState::Ready,
        deadline_secs: Some(30),
        satisfied: false,
        started_at_secs: Some(1000),
    };
    let entry = make_entry(0, CallKind::Start, Some(barrier));
    insert_action_log_entry(&db, &op, &app_name("myapp"), "start", &entry).unwrap();

    let satisfied_entry = ActionLogEntry {
        call_index: 0,
        call_kind: CallKind::Start,
        resources: vec![dep("app", "web")],
        barrier: Some(BarrierRecord {
            required_state: LifecycleState::Ready,
            deadline_secs: Some(30),
            satisfied: true,
            started_at_secs: Some(1000),
        }),
    };
    insert_action_log_entry(&db, &op, "myapp", "start", &satisfied_entry).unwrap();

    let loaded = load_action_log(&db, &op).unwrap();
    assert_eq!(loaded.len(), 1, "INSERT OR REPLACE should not duplicate");
    assert!(loaded[0].barrier.as_ref().unwrap().satisfied);
}

// r[verify history.action-log.entries]
#[test]
fn action_log_multiple_entries_ordered_by_call_index() {
    let db = Db::open_in_memory().unwrap();
    let op = OperationId("op-1".into());
    for i in [2usize, 0, 1] {
        insert_action_log_entry(
            &db,
            &op,
            &app_name("myapp"),
            "start",
            &make_entry(i, CallKind::Start, None),
        )
        .unwrap();
    }
    let loaded = load_action_log(&db, &op).unwrap();
    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[0].call_index, 0);
    assert_eq!(loaded[1].call_index, 1);
    assert_eq!(loaded[2].call_index, 2);
}

// r[verify history.action-log.entries]
#[test]
fn action_log_scoped_to_operation_id() {
    let db = Db::open_in_memory().unwrap();
    let op1 = OperationId("op-1".into());
    let op2 = OperationId("op-2".into());
    insert_action_log_entry(
        &db,
        &op1,
        &app_name("myapp"),
        "start",
        &make_entry(0, CallKind::Start, None),
    )
    .unwrap();
    insert_action_log_entry(
        &db,
        &op2,
        &app_name("myapp"),
        "start",
        &make_entry(0, CallKind::Stop, None),
    )
    .unwrap();

    let loaded1 = load_action_log(&db, &op1).unwrap();
    assert_eq!(loaded1.len(), 1);
    assert!(matches!(loaded1[0].call_kind, CallKind::Start));

    let loaded2 = load_action_log(&db, &op2).unwrap();
    assert_eq!(loaded2.len(), 1);
    assert!(matches!(loaded2[0].call_kind, CallKind::Stop));
}
