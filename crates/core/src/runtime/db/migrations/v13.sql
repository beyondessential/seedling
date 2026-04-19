CREATE INDEX IF NOT EXISTS idx_world_observations_instance
    ON world_observations (instance_id, recorded_at);

CREATE INDEX IF NOT EXISTS idx_autonomous_operations_instance
    ON autonomous_operations (instance_id, recorded_at);

CREATE INDEX IF NOT EXISTS idx_action_log_operation
    ON action_log (operation_id, call_index);

CREATE INDEX IF NOT EXISTS idx_faults_active_app
    ON faults (app, cleared_at)
    WHERE cleared_at IS NULL;
