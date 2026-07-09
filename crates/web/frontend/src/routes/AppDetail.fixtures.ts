// Fixture builders shared by the AppDetail test files.
import type {
  AppAction,
  AppDetail,
  AppParam,
  AppResource,
  ContainerSummary,
  FaultRecord,
  HealthcheckSummary,
} from "../lib/types";

export function makeContainer(
  overrides: Partial<ContainerSummary> = {},
): ContainerSummary {
  return {
    image: "docker.io/library/nginx:1.27",
    command: null,
    args: null,
    env: {},
    volume_mounts: {},
    on_exit: "restart",
    memory: null,
    cpus: null,
    extra_caps: [],
    writable_rootfs: false,
    pids_limit: null,
    workdir: null,
    healthcheck: null,
    ...overrides,
  };
}

export const healthcheck: HealthcheckSummary = {
  kind: "command",
  cmd: ["curl", "-f", "http://localhost/health"],
  interval_secs: 10,
  timeout_secs: 5,
  retries: 3,
  start_period_secs: 30,
  on_failure: "restart",
};

export function makeWebDeployment(
  overrides: Partial<AppResource> = {},
): AppResource {
  return {
    name: "web",
    type: "deployment",
    instances: [{ id: "inst-web-0", display_name: "web-0", lifecycle: "ready" }],
    faults: [],
    scale: { low: 1, high: 5, current: 2 },
    def: {
      kind: "deployment",
      container: makeContainer(),
      pod: {
        service_mounts: [],
        http_bindings: ["web:80"],
        tcp_bindings: [],
        udp_bindings: [],
      },
      scale: { low: 1, high: 5 },
      on_update: "restart",
      on_terminate: "stop",
      description: null,
    },
    ...overrides,
  };
}

export const dataVolume: AppResource = {
  name: "data",
  type: "volume",
  instances: [{ id: "vol-data", display_name: "data", lifecycle: "active" }],
  faults: [],
  def: {
    kind: "volume",
    readonly: false,
    tmpfs: false,
    writes: {},
    exported: false,
    export_description: null,
    description: null,
  },
};

export const params: AppParam[] = [
  {
    name: "domain",
    value: "example.com",
    is_set: true,
    secret: false,
    kind: "string",
    required: false,
    description: "Public hostname",
    default_value: null,
  },
  {
    name: "admin_password",
    value: null,
    is_set: true,
    secret: true,
    kind: "password",
    required: true,
    description: null,
    default_value: null,
  },
  {
    name: "workers",
    value: null,
    is_set: false,
    secret: false,
    kind: "string",
    required: false,
    description: null,
    default_value: "4",
  },
];

export const installAction: AppAction = {
  name: "on_install",
  description: "Set up the database",
  kind: "install",
  params: {},
  schedules: [],
};

export const backupAction: AppAction = {
  name: "backup",
  description: "Back up the data volume",
  kind: "action",
  params: {},
  schedules: [
    {
      cronexpr: "0 3 * * *",
      last_fired_at: "2026-07-08T03:00:00Z",
      next_fire_at: "2026-07-09T03:00:00Z",
    },
  ],
};

export const reindexAction: AppAction = {
  name: "reindex",
  description: "Rebuild the search index",
  kind: "action",
  params: {
    depth: {
      kind: "string",
      required: false,
      description: "How deep to go",
      default_value: "2",
    },
  },
  schedules: [],
};

export const dbShellAction: AppAction = {
  name: "db-shell",
  description: null,
  kind: "shell",
  params: {},
  schedules: [],
};

export const psqlShellAction: AppAction = {
  name: "psql",
  description: null,
  kind: "shell",
  params: {
    database: {
      kind: "string",
      required: true,
      description: "Database to connect to",
      default_value: "main",
    },
  },
  schedules: [],
};

export function makeFault(overrides: Partial<FaultRecord> = {}): FaultRecord {
  return {
    id: "fault-1",
    app: "myapp",
    kind: "container_crashed",
    resource_type: "deployment",
    resource_name: "web",
    instance_id: "inst-web-0",
    timestamp: "2026-07-09T08:00:00Z",
    description: "container exited with status 137",
    ...overrides,
  };
}

export function makeDetail(overrides: Partial<AppDetail> = {}): AppDetail {
  return {
    status: "running",
    generation: 4,
    description: "A **testing** app",
    faults: [],
    resources: [makeWebDeployment(), dataVolume],
    dynamic_resources: [],
    stopped_resources: [],
    params,
    unknown_params: [],
    actions: [installAction, backupAction, reindexAction, dbShellAction],
    ...overrides,
  };
}

/** Baseline fixture map for mounting AppDetail at /apps/myapp. */
export function baseFixtures(detail: AppDetail): Record<string, unknown> {
  return {
    "/apps/show": detail,
    "/volumes/external/list": [],
    "/images/list": { images: [] },
    "/images/pins/list": { pins: [] },
    "/volumes/site/list": [],
  };
}
