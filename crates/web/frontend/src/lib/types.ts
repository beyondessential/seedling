export type AppStatus =
  | "not_installed"
  | "uninstalling"
  | "deregistering"
  | "operating"
  | "running"
  | "degraded"
  | "faulted";

export interface AppSummary {
  name: string;
  status: AppStatus;
  action_name?: string;
}

export interface FaultRecord {
  id: string;
  app?: string;
  kind: string;
  resource_type?: string;
  resource_name?: string;
  instance_id?: string;
  timestamp: string;
  description: string;
}

export interface ResourceInstance {
  id: string;
  display_name: string;
  lifecycle: string;
  transition_time?: string;
}

export interface ScaleBounds {
  low: number;
  high: number;
  current: number;
}

export interface AppResource {
  name: string;
  type: string;
  instances: ResourceInstance[];
  faults: FaultRecord[];
  scale?: ScaleBounds;
}

export interface AppParam {
  name: string;
  value: string | null;
  kind: string;
  required: boolean;
  description: string | null;
  default_value: string | null;
}

export interface AppAction {
  name: string;
  description: string | null;
  kind: "action" | "shell" | "install" | "lifecycle";
  params: Record<string, InstallRequirement>;
  schedules?: string[];
}

export interface CurrentOperation {
  action_name: string;
  source_generation: number;
  target_generation: number;
  barrier: {
    resources: string[];
    required_state: string;
    deadline_secs: number;
    elapsed_secs: number;
  } | null;
}

export interface InstallRequirement {
  kind: string;
  required: boolean;
  description: string;
  default_value: string | null;
}

export interface AppDetail {
  status: AppStatus;
  generation: number;
  faults: FaultRecord[];
  resources: AppResource[];
  params: AppParam[];
  unknown_params: AppParam[];
  actions: AppAction[];
  current_operation?: CurrentOperation;
}

export interface SeedlingEvent {
  type: string;
  timestamp: string;
  actor?: Actor;
  // App-scoped events
  app?: string;
  // AppRegistered / AppUpdated
  generation?: number;
  previous_generation?: number;
  // ParamSet / ParamUnset
  name?: string;
  // OperationStarted / OperationCompleted / OperationFailed
  action_name?: string;
  operation_id?: string;
  source_generation?: number;
  target_generation?: number;
  trigger?: string;
  error?: string;
  // FaultFiled / FaultCleared
  id?: string;
  kind?: string;
  resource_type?: string;
  resource_name?: string;
  instance_id?: string;
  description?: string;
  // ResourceStateChanged
  state?: string;
  // ScaleChanged
  deployment?: string;
  scale?: number;
  previous_scale?: number;
  bounds_low?: number;
  bounds_high?: number;
  // ForwardStarted / ForwardStopped
  forward_id?: string;
  service?: string;
  port?: number;
  // ShellExited
  session_id?: string;
  exit_code?: number;
  // ServerBusy
  reason?: string;
}

export interface Actor {
  kind?: string;
  id?: string;
  display?: string;
  session?: string;
}

export interface ConnectRequest {
  token?: string;
  password?: string;
}

export interface ConnectResponse {
  token: string;
  actor: Actor;
  wt_url: string;
  cert_hashes: string[];
}

export interface OiRequest {
  method: string;
  actor?: Actor;
  params?: unknown;
}

export interface OiError {
  code: string;
  message: string;
}

export type OiResult =
  | { ok: true; value: unknown }
  | { ok: false; error: OiError };
