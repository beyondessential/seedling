export type AppStatus =
  | "not_installed"
  | "installing"
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
  has_stopped_resources?: boolean;
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

export interface ContainerSummary {
  image: string | null;
  command: string[] | null;
  args: string[] | null;
  env: Record<string, string>;
  volume_mounts: Record<string, { kind: "volume"; name: string | null } | { kind: "external_volume"; name: string }>;
  on_exit: string;
  memory: string | null;
  cpus: number | null;
  extra_caps: string[];
  writable_rootfs: boolean;
  pids_limit: number | null;
  workdir: string | null;
  healthcheck: HealthcheckSummary | null;
}

export interface HealthcheckSummary {
  kind: "command";
  cmd: string[] | null;
  interval_secs: number;
  timeout_secs: number;
  retries: number;
  start_period_secs: number;
  on_failure: "none" | "kill" | "restart" | "stop";
}

export interface PodSummary {
  service_mounts: string[];
  http_bindings: string[];
  tcp_bindings: string[];
  udp_bindings: string[];
}

export type ResourceDef =
  | { kind: "service"; http: boolean }
  | { kind: "http_service"; service: string; port: number }
  | { kind: "ingress"; service: string; hostname: string; port: number; tls: boolean; dtls: boolean; http_terminate: "http1" | "http2" | null; redirect: { port: number; code: number } | null }
  | { kind: "deployment"; container: ContainerSummary; pod: PodSummary; scale: { low: number; high: number }; on_update: string; on_terminate: string }
  | { kind: "job"; container: ContainerSummary; pod: PodSummary; deadline: number | null }
  | { kind: "volume"; readonly: boolean; tmpfs: boolean; writes: Record<string, string>; exported: boolean; export_description: string | null }
  | { kind: "external_volume" };

export interface StoppedResource {
  kind: string;
  name: string;
}

export interface AppResource {
  name: string;
  type: string;
  instances: ResourceInstance[];
  faults: FaultRecord[];
  scale?: ScaleBounds;
  def?: ResourceDef;
  stopped?: boolean;
}

export interface AppParam {
  name: string;
  value: string | null;
  is_set: boolean;
  secret: boolean;
  kind: string;
  required: boolean;
  description: string | null;
  default_value: string | null;
}

export interface ActionSchedule {
  cronexpr: string;
  last_fired_at: string | null;
  next_fire_at: string | null;
}

export interface AppAction {
  name: string;
  description: string | null;
  kind: "action" | "shell" | "install" | "lifecycle";
  params: Record<string, InstallRequirement>;
  schedules: ActionSchedule[];
}

export interface CurrentOperation {
  action_name: string;
  source_generation: number;
  target_generation: number;
  barrier: {
    resources: string[];
    required_state: string;
    /** `null` when the barrier has no deadline (e.g. `.terminated_eventually()`). */
    deadline_secs: number | null;
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
  stopped_resources: StoppedResource[];
  params: AppParam[];
  unknown_params: AppParam[];
  actions: AppAction[];
  current_operation?: CurrentOperation;
}

export interface LogEntry {
  timestamp: string;
  message: string;
  unit: string;
  stream: "stdout" | "stderr";
  app?: string;
  resource_kind?: string;
  resource?: string;
  instance?: string;
  infra?: string;
}

export interface LogStreamParams {
  app?: string;
  infra?: string;
  resource?: string;
  instance?: string;
  follow?: boolean;
  tail?: number;
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
  // AppPhaseChanged
  phase?: string;
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
  // ShellStarted / ShellExited
  session_id?: string;
  exit_code?: number;
  // ForwardStarted / ForwardStopped
  forward_id?: string;
  service?: string;
  port?: number;
  // WebSessionStarted / WebSessionStopped (web-layer events)
  // ServerBusy
  reason?: string;
  // HeldVolumeCreated / HeldVolumeDeleted
  held_id?: string;
  volume_name?: string;
}

export interface Actor {
  kind?: string;
  id?: string;
  display?: string;
  session?: string;
}

export interface WebSession {
  id: string;
  connected_at: string;
  /**
   * RFC 3339 timestamp of the most recent heartbeat from this session.
   * Defaults to `connected_at` when no heartbeat has arrived yet. Sessions
   * older than the stale cutoff are reaped server-side and never appear here.
   */
  last_seen: string;
  actor_kind: string | null;
  actor_id: string | null;
  actor_display: string | null;
}

export interface ShellSession {
  session_id: string;
  app: string;
  name: string;
  opened_at: string;
  actor?: Actor;
}

export interface ForwardSession {
  forward_id: string;
  app: string;
  service: string;
  port: number;
  proto: string;
  opened_at: string;
  actor?: Actor;
}

export interface ActorActivity {
  actor_kind: string;
  actor_id: string;
  actor_display: string | null;
  /** RFC 3339 timestamp of the most recent attributed event for this actor. */
  last_seen: string;
  /** Short human-readable summary of the most recent attributed event. */
  last_action: string;
}

export interface ConnectedClients {
  web: WebSession[];
  shells: ShellSession[];
  forwards: ForwardSession[];
  actors: ActorActivity[];
}

export interface AuthorizedKey {
  fingerprint: string;
  label: string;
  added_at: string;
}

export interface SiteVolume {
  name: string;
  kind: "managed" | "bind" | "snapshot";
  created_at: string;
  host_path?: string;
  source?: string;
}

export interface ExportedVolume {
  app: string;
  volume_name: string;
  description?: string;
}

export interface AppVolume {
  app: string;
  volume_name: string;
  exported: boolean;
  description?: string;
}

export interface DeclaredExternalVolume {
  app: string;
  name: string;
}

export interface ExternalMapping {
  app: string;
  external_name: string;
  read_only: boolean;
  target: Exclude<VolumeRef, { kind: "held" }>;
}

export interface HeldVolume {
  id: string;
  app: string;
  volume_name: string;
  display_name: string;
  reason: string;
  held_at: string;
}

export type VolumeRef =
  | { kind: "site"; name: string }
  | { kind: "app"; app: string; volume: string }
  | { kind: "held"; id: string };

export type SiteServiceProtocol = "tcp" | "udp" | "http";

export interface SiteServiceEndpoint {
  service_port: number;
  protocol: SiteServiceProtocol;
  remote_host: string;
  remote_port: number;
}

export interface SiteService {
  name: string;
  description?: string;
  created_at: string;
  endpoints: SiteServiceEndpoint[];
}

export interface ExportedService {
  app: string;
  service_name: string;
  http: boolean;
  description?: string;
}

export interface AppService {
  app: string;
  service_name: string;
  http: boolean;
  exported: boolean;
  description?: string;
}

export interface DeclaredExternalService {
  app: string;
  name: string;
}

export type ServiceRef =
  | { kind: "site"; name: string }
  | { kind: "app"; app: string; service: string };

export interface ExternalServiceMapping {
  app: string;
  external_name: string;
  target: ServiceRef;
}

export interface BackupApp {
  app: string;
}

export const BACKUP_SCHEDULES = ["every hour", "twice a day", "every day"] as const;
export type BackupSchedule = (typeof BACKUP_SCHEDULES)[number];

export interface BackupStrategy {
  name: string;
  via: string;
  schedule: BackupSchedule;
  volumes: string[];
  last_fired_at: string | null;
  next_fire_at: string | null;
}

export interface BackupRunResult {
  volume: string;
  operation_id: string;
}

export interface PlanDiffEntry {
  resource_type: string;
  resource_name: string;
  change: "added" | "removed" | "modified";
  fields?: string[];
}

export interface PlanResponse {
  diff?: PlanDiffEntry[];
  on_change_would_fire?: string[];
  errors?: string[];
}

export interface TemplateSummary {
  name: string;
  description: string | null;
  created_at: string;
}

export interface Template {
  name: string;
  body: string;
  description: string | null;
  created_at: string;
}

export interface TemplatePreviewResource {
  name: string;
  type: string;
  def?: ResourceDef;
  scale?: { low: number; high: number };
  export?: { exported: boolean; description?: string };
}

export interface TemplatePreview {
  resources: TemplatePreviewResource[];
  params: AppParam[];
  actions: AppAction[];
  script_error: string | null;
}

/**
 * `manifest`: the digest refers to this image's own image manifest.
 * `manifest_list`: the digest refers to the multi-arch manifest list the
 *   image was pulled from (so the same local image can reach that digest
 *   only by traversing the list).
 * `unknown`: the container runtime didn't report the image's own manifest
 *   digest, so we can't classify.
 */
export type ImageDigestKind = "manifest" | "manifest_list" | "unknown";

export interface ImageDigest {
  reference: string;
  kind: ImageDigestKind;
}

export interface ImageSummary {
  image_id: string;
  tags: string[];
  digests: ImageDigest[];
  /** The image's own manifest digest, as `"sha256:..."`, when known. */
  manifest_digest: string | null;
  size_bytes: number;
  created_at: string;
  last_used_at: string;
  in_use: boolean;
  pinned_by: string[];
}

export interface ImagePin {
  app: string;
  reference: string;
  pinned_at: string;
  /**
   * RFC 3339 timestamp when the reconciler will auto-delete this pin, or
   * `null` for an indefinite pin. Set by the post-update reconciliation
   * rule when the owning script's probe was dirty and couldn't confirm
   * the pin's reference.
   */
  expires_at: string | null;
}

export type HandlerKind =
  | "install"
  | "start"
  | "action"
  | "shell"
  | "param_change";

export interface HandlerProbe {
  name: string;
  kind: HandlerKind;
  images: string[];
  error: string | null;
  skipped_reason: string | null;
}

export interface DiscoverResponse {
  per_handler: HandlerProbe[];
  all_images: string[];
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

// ---------------------------------------------------------------------------
// TLS certificate management
// ---------------------------------------------------------------------------

export type TlsDnsProviderKind = "route53";

export interface TlsDnsProvider {
  name: string;
  kind: TlsDnsProviderKind;
  created_at: number;
  updated_at: number;
}

export interface TlsDnsProvidersResponse {
  providers: TlsDnsProvider[];
}

export type TlsStrategy = "acme_dns" | "manual";

export interface TlsPolicyAcmeDns {
  hostname: string;
  strategy: "acme_dns";
  dns_provider: string;
  updated_at: number;
}

export interface TlsPolicyManual {
  hostname: string;
  strategy: "manual";
  cert_id: number;
  updated_at: number;
}

export type TlsPolicy = TlsPolicyAcmeDns | TlsPolicyManual;

export interface TlsPoliciesResponse {
  policies: TlsPolicy[];
}

export type TlsCertState = "csr_pending" | "active" | "superseded" | "failed";
export type TlsCertOrigin = "manual" | "csr" | "acme_dns";
export type TlsKeyType = "ecdsa_p256";

export interface TlsCertificate {
  id: number;
  hostname: string;
  state: TlsCertState;
  origin: TlsCertOrigin;
  key_type: TlsKeyType;
  issuer: string | null;
  not_before: number | null;
  not_after: number | null;
  serial: string | null;
  self_signed: boolean;
  note: string | null;
  acme_account_id: number | null;
  created_at: number;
  updated_at: number;
}

export interface TlsCertificatesResponse {
  certificates: TlsCertificate[];
}
