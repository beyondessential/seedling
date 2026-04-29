import AddIcon from "@mui/icons-material/Add";
import ArticleIcon from "@mui/icons-material/Article";
import CameraAltIcon from "@mui/icons-material/CameraAlt";
import CasinoIcon from "@mui/icons-material/Casino";
import CheckIcon from "@mui/icons-material/Check";
import ClearIcon from "@mui/icons-material/Clear";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutlineOutlined";
import EditIcon from "@mui/icons-material/Edit";
import PauseIcon from "@mui/icons-material/Pause";
import PlayArrowIcon from "@mui/icons-material/PlayArrow";
import RefreshIcon from "@mui/icons-material/Refresh";
import RemoveIcon from "@mui/icons-material/Remove";
import RestoreIcon from "@mui/icons-material/Restore";
import TerminalIcon from "@mui/icons-material/Terminal";
import VisibilityIcon from "@mui/icons-material/Visibility";
import VisibilityOffIcon from "@mui/icons-material/VisibilityOff";
import {
  Alert,
  Box,
  Button,
  Chip,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogContentText,
  DialogTitle,
  Divider,
  FormControl,
  FormHelperText,
  IconButton,
  InputAdornment,
  InputLabel,
  MenuItem,
  Paper,
  Select,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  TextField,
  Tooltip,
  Typography,
} from "@mui/material";
import { useCallback, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  IconActionButton,
  OutlinedActionButton,
  SolidActionButton,
} from "../components/ActionButton";
import {
  ImageReferencesCell,
  primaryReference,
} from "../components/ImageReferences";
import { MapVolumeDialog } from "../components/MapVolumeDialog";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useSafetyMode } from "../components/SafetyModeProvider";
import { useSessionContext } from "../components/SessionProvider";
import { SnapshotVolumeDialog } from "../components/SnapshotVolumeDialog";
import { TlsHostnamesTable } from "../components/TlsHostnamesTable";
import { useOiAction } from "../hooks/useOiAction";
import { useOiQuery } from "../hooks/useOi";
import { useEventRefresh } from "../hooks/useEventRefresh";
import { isStrongPassword, passwordScore } from "../lib/passwordStrength";
import { statusColor, statusLabel } from "../lib/status";
import type {
  ActionSchedule,
  AppAction,
  AppDetail,
  AppParam,
  AppResource,
  AppStatus,
  DiscoverResponse,
  ExternalMapping,
  FaultRecord,
  HandlerProbe,
  HealthcheckSummary,
  ImagePin,
  ImageSummary,
  InstallRequirement,
  ResourceDef,
  SeedlingEvent,
  SiteVolume,
} from "../lib/types";

function lifecycleColor(
  state: string,
): "success" | "warning" | "error" | "default" {
  if (state === "ready" || state === "active") return "success";
  if (state === "failed") return "error";
  if (state === "excluded") return "warning";
  return "default";
}

type HealthcheckState = "passing" | "failing" | "starting" | "idle";

function healthcheckState(
  lifecycle: string,
  instanceId: string,
  faults: FaultRecord[],
): HealthcheckState {
  const failing = faults.some(
    (f) => f.kind === "health_check_failed" && f.instance_id === instanceId,
  );
  if (failing) return "failing";
  if (lifecycle === "ready") return "passing";
  if (lifecycle === "running") return "starting";
  return "idle";
}

function healthcheckChipColor(
  state: HealthcheckState,
): "success" | "warning" | "error" | "default" {
  switch (state) {
    case "passing":
      return "success";
    case "failing":
      return "error";
    case "starting":
      return "warning";
    case "idle":
      return "default";
  }
}

// w[impl routes.apps.healthcheck-indicator]
function HealthcheckIndicator({
  hc,
  lifecycle,
  instanceId,
  faults,
}: {
  hc: HealthcheckSummary;
  lifecycle: string;
  instanceId: string;
  faults: FaultRecord[];
}) {
  const state = healthcheckState(lifecycle, instanceId, faults);
  const cmdPreview =
    hc.kind === "command" && hc.cmd ? hc.cmd.join(" ") : hc.kind;
  const truncated =
    cmdPreview.length > 80 ? `${cmdPreview.slice(0, 77)}…` : cmdPreview;
  const tooltip = [
    `healthcheck (${hc.kind}): ${state}`,
    `on_failure: ${hc.on_failure}`,
    truncated,
  ].join("\n");
  const label =
    state === "failing"
      ? "unhealthy"
      : state === "starting"
        ? "starting"
        : state === "passing"
          ? "healthy"
          : "check";
  return (
    <Tooltip title={<span style={{ whiteSpace: "pre-line" }}>{tooltip}</span>}>
      <Chip
        label={label}
        color={healthcheckChipColor(state)}
        size="small"
        variant="outlined"
        sx={{
          fontSize: "0.65rem",
          height: 18,
          "& .MuiChip-label": { px: 0.75 },
        }}
      />
    </Tooltip>
  );
}

function containerHealthcheck(
  def: ResourceDef | undefined,
): HealthcheckSummary | null {
  if (!def) return null;
  if (def.kind !== "deployment" && def.kind !== "job") return null;
  return def.container.healthcheck ?? null;
}

function healthcheckTooltip(hc: HealthcheckSummary): string {
  const lines: string[] = [];
  lines.push(`kind: ${hc.kind}`);
  if (hc.kind === "command" && hc.cmd) {
    const joined = hc.cmd.join(" ");
    lines.push(
      `cmd: ${joined.length > 120 ? `${joined.slice(0, 117)}…` : joined}`,
    );
  }
  lines.push(
    `interval=${hc.interval_secs}s · timeout=${hc.timeout_secs}s · retries=${hc.retries} · start_period=${hc.start_period_secs}s`,
  );
  lines.push(`on_failure: ${hc.on_failure}`);
  return lines.join("\n");
}

function healthcheckChipLabel(hc: HealthcheckSummary): string {
  const base = `healthcheck (${hc.kind})`;
  switch (hc.on_failure) {
    case "none":
      return base;
    case "restart":
      return `${base}, restart on failure`;
    case "kill":
      return `${base}, kill on failure`;
    case "stop":
      return `${base}, stop on failure`;
  }
}

function FaultList({
  faults,
  showApp,
}: {
  faults: FaultRecord[];
  showApp?: boolean;
}) {
  if (faults.length === 0) return null;
  return (
    <Stack spacing={1}>
      {faults.map((f) => (
        <Alert key={f.id} severity="error" sx={{ fontFamily: "monospace" }}>
          <Box
            sx={{
              display: "flex",
              justifyContent: "space-between",
              gap: 2,
              flexWrap: "wrap",
            }}
          >
            <Box>
              {showApp && f.app && (
                <>
                  <Link to={`/apps/${f.app}`} style={{ color: "inherit" }}>
                    {f.app}
                  </Link>
                  {" · "}
                </>
              )}
              <strong>{f.kind}</strong>
              {f.resource_name && ` · ${f.resource_type}/${f.resource_name}`}
              {f.instance_id && ` (${f.instance_id.slice(0, 12)})`}
              {" — "}
              {f.description}
            </Box>
            <Typography
              variant="caption"
              sx={{
                color: "text.secondary",
                whiteSpace: "nowrap",
                alignSelf: "center",
              }}
            >
              {new Date(f.timestamp).toLocaleString()}
            </Typography>
          </Box>
        </Alert>
      ))}
    </Stack>
  );
}

function ResourceDefDetail({ def }: { def: ResourceDef }) {
  if (def.kind === "ingress") {
    const scheme = def.tls ? "https" : "http";
    const url = `${scheme}://${def.hostname}:${def.port}`;
    return (
      <Box
        sx={{
          mt: 0.5,
          display: "flex",
          gap: 0.5,
          flexWrap: "wrap",
          alignItems: "center",
        }}
      >
        <Typography variant="caption" sx={{ fontFamily: "monospace", mr: 0.5 }}>
          {url}
        </Typography>
        {def.http_terminate && (
          <Chip label={def.http_terminate} size="small" variant="outlined" />
        )}
        {def.dtls && <Chip label="dtls" size="small" variant="outlined" />}
        {def.redirect && (
          <Chip
            label={`redirect :${def.redirect.port} (${def.redirect.code})`}
            size="small"
            variant="outlined"
          />
        )}
      </Box>
    );
  }
  if (def.kind === "service") {
    if (!def.http) return null;
    return (
      <Chip label="http" size="small" variant="outlined" sx={{ mt: 0.5 }} />
    );
  }
  if (def.kind === "http_service") {
    return (
      <Typography
        variant="caption"
        sx={{ fontFamily: "monospace", display: "block", mt: 0.5 }}
      >
        {def.service}:{def.port}
      </Typography>
    );
  }
  if (def.kind === "deployment" || def.kind === "job") {
    const bindings = [
      ...def.pod.http_bindings.map((b) => `http: ${b}`),
      ...def.pod.tcp_bindings.map((b) => `tcp: ${b}`),
      ...def.pod.udp_bindings.map((b) => `udp: ${b}`),
    ];
    return (
      <Box
        sx={{
          mt: 0.5,
          display: "flex",
          gap: 0.5,
          flexWrap: "wrap",
          alignItems: "center",
        }}
      >
        {def.container.image && (
          <Typography
            variant="caption"
            sx={{
              fontFamily: "monospace",
              opacity: 0.8,
              maxWidth: 400,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
            title={def.container.image}
          >
            {def.container.image}
          </Typography>
        )}
        {bindings.map((b) => (
          <Chip key={b} label={b} size="small" variant="outlined" />
        ))}
        {def.container.memory && (
          <Chip
            label={`mem: ${def.container.memory}`}
            size="small"
            variant="outlined"
          />
        )}
        {def.container.cpus != null && (
          <Chip
            label={`cpu: ${def.container.cpus}`}
            size="small"
            variant="outlined"
          />
        )}
        {def.kind === "job" && def.deadline != null && (
          <Chip
            label={`deadline: ${def.deadline}s`}
            size="small"
            variant="outlined"
          />
        )}
        {def.container.healthcheck && (
          <Tooltip
            title={
              <span style={{ whiteSpace: "pre-line" }}>
                {healthcheckTooltip(def.container.healthcheck)}
              </span>
            }
          >
            <Chip
              label={healthcheckChipLabel(def.container.healthcheck)}
              size="small"
              variant="outlined"
            />
          </Tooltip>
        )}
      </Box>
    );
  }
  if (def.kind === "volume") {
    const chips = [
      def.tmpfs && "tmpfs",
      !def.tmpfs && "persistent",
      def.readonly && "readonly",
      def.exported && (def.export_description ?? "exported"),
    ].filter(Boolean) as string[];
    if (chips.length === 0) return null;
    return (
      <Box sx={{ mt: 0.5, display: "flex", gap: 0.5, flexWrap: "wrap" }}>
        {chips.map((c) => (
          <Chip key={c} label={c} size="small" variant="outlined" />
        ))}
      </Box>
    );
  }
  return null;
}

const STOPPABLE_KINDS = new Set(["deployment", "job", "ingress"]);

function ResourcesSection({
  appName,
  resources,
  onRefresh,
}: {
  appName: string;
  resources: AppResource[];
  onRefresh: () => void;
}) {
  const navigate = useNavigate();
  const { execute, loading: scaling } = useOiAction();
  const { execute: executeRestart, loading: restarting } = useOiAction();
  const { execute: executeStop, loading: stopping } = useOiAction();
  const { openVolumeShell } = useSessionContext();
  const { mode } = useSafetyMode();
  // w[impl volumes.shell-ui.read-only]
  const shellReadOnly = mode === "read";
  const [snapshotTarget, setSnapshotTarget] = useState<{
    source: string;
    label: string;
  } | null>(null);

  const scale = async (deploymentName: string, value: number) => {
    try {
      await execute("/apps/scale", {
        app: appName,
        deployment: deploymentName,
        scale: value,
      });
      onRefresh();
    } catch {
      // errors surfaced by useOiAction globally
    }
  };

  const restart = async (deploymentName: string) => {
    try {
      await executeRestart("/apps/restart", {
        app: appName,
        deployment: deploymentName,
      });
    } catch {
      // errors surfaced by useOiAction globally
    }
  };

  const stopResource = async (kind: string, resourceName: string) => {
    try {
      await executeStop("/apps/resource/stop", {
        app: appName,
        kind,
        name: resourceName,
      });
      onRefresh();
    } catch {
      // errors surfaced by useOiAction globally
    }
  };

  const unstopResource = async (kind: string, resourceName: string) => {
    try {
      await executeStop("/apps/resource/unstop", {
        app: appName,
        kind,
        name: resourceName,
      });
      onRefresh();
    } catch {
      // errors surfaced by useOiAction globally
    }
  };

  const snapshotDialog = snapshotTarget && (
    <SnapshotVolumeDialog
      key={snapshotTarget.source}
      source={snapshotTarget.source}
      sourceLabel={snapshotTarget.label}
      onClose={() => setSnapshotTarget(null)}
      onSuccess={onRefresh}
    />
  );

  if (resources.length === 0)
    return (
      <>
        <Typography
          sx={{
            color: "text.secondary",
          }}
        >
          No resources.
        </Typography>
        {snapshotDialog}
      </>
    );
  return (
    <Stack spacing={2}>
      {resources.map((r) => (
        <Box key={`${r.type}/${r.name}`}>
          <Box sx={{ display: "flex", alignItems: "center", gap: 1, mb: 0.5 }}>
            <Typography variant="subtitle2">{r.name}</Typography>
            <Typography
              variant="caption"
              sx={{
                color: "text.secondary",
              }}
            >
              {r.type}
            </Typography>
            {r.stopped && (
              <Chip
                label="stopped"
                size="small"
                color="warning"
                variant="outlined"
                sx={{
                  fontSize: "0.65rem",
                  height: 18,
                  "& .MuiChip-label": { px: 0.75 },
                }}
              />
            )}
            {r.dynamic && (
              <Chip
                label={r.anonymous ? "dynamic · anonymous" : "dynamic"}
                size="small"
                color="info"
                variant="outlined"
                sx={{
                  fontSize: "0.65rem",
                  height: 18,
                  "& .MuiChip-label": { px: 0.75 },
                }}
              />
            )}
            {(r.type === "deployment" || r.type === "job") && (
              <IconActionButton
                safety="read"
                tooltip="View resource logs"
                onClick={() =>
                  navigate(`/apps/${appName}/logs?resource=${r.name}`)
                }
              >
                <ArticleIcon sx={{ fontSize: 14 }} />
              </IconActionButton>
            )}
            {!r.dynamic && r.scale && (
              <>
                <Typography
                  variant="caption"
                  sx={{
                    color: "text.secondary",
                  }}
                >
                  · scale
                </Typography>
                <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
                  <IconActionButton
                    safety="write"
                    tooltip="Scale down"
                    onClick={() => void scale(r.name, r.scale!.current - 1)}
                    disabled={scaling || r.scale.current <= r.scale.low}
                  >
                    <RemoveIcon sx={{ fontSize: 14 }} />
                  </IconActionButton>
                  <Typography variant="caption">{r.scale.current}</Typography>
                  <IconActionButton
                    safety="write"
                    tooltip="Scale up"
                    onClick={() => void scale(r.name, r.scale!.current + 1)}
                    disabled={scaling || r.scale.current >= r.scale.high}
                  >
                    <AddIcon sx={{ fontSize: 14 }} />
                  </IconActionButton>
                  <Typography
                    variant="caption"
                    sx={{
                      color: "text.secondary",
                    }}
                  >
                    [{r.scale.low}–{r.scale.high}]
                  </Typography>
                </Box>
              </>
            )}
            {!r.dynamic && r.type === "deployment" && (
              <IconActionButton
                safety="write"
                tooltip="Restart deployment"
                onClick={() => void restart(r.name)}
                disabled={restarting}
              >
                <RefreshIcon sx={{ fontSize: 14 }} />
              </IconActionButton>
            )}
            {/* w[volumes.shell-ui] */}
            {/* w[impl volumes.shell-ui.read-only] */}
            {!r.dynamic && r.type === "volume" && (
              <>
                <IconActionButton
                  safety="read"
                  color={shellReadOnly ? undefined : "warning"}
                  tooltip={
                    shellReadOnly ? "Open shell (read-only)" : "Open shell"
                  }
                  onClick={() =>
                    openVolumeShell(
                      [{ kind: "app", app: appName, volume: r.name }],
                      `${appName}.${r.name}`,
                      { readOnly: shellReadOnly },
                    )
                  }
                >
                  <TerminalIcon sx={{ fontSize: 14 }} />
                </IconActionButton>
                <IconActionButton
                  safety="write"
                  tooltip="Snapshot"
                  onClick={() =>
                    setSnapshotTarget({
                      source: `${appName}/${r.name}`,
                      label: `${appName}/${r.name}`,
                    })
                  }
                >
                  <CameraAltIcon sx={{ fontSize: 14 }} />
                </IconActionButton>
              </>
            )}
            {!r.dynamic &&
              STOPPABLE_KINDS.has(r.type) &&
              (r.stopped ? (
                <IconActionButton
                  safety="write"
                  tooltip="Unstop resource"
                  onClick={() => void unstopResource(r.type, r.name)}
                  disabled={stopping}
                >
                  <PlayArrowIcon sx={{ fontSize: 14 }} />
                </IconActionButton>
              ) : (
                <IconActionButton
                  safety="write"
                  tooltip="Stop resource"
                  onClick={() => void stopResource(r.type, r.name)}
                  disabled={stopping}
                >
                  <PauseIcon sx={{ fontSize: 14 }} />
                </IconActionButton>
              ))}
          </Box>
          <FaultList faults={r.faults} />
          {r.def && <ResourceDefDetail def={r.def} />}
          <TableContainer component={Paper} variant="outlined" sx={{ mt: 0.5 }}>
            <Table size="small">
              <TableHead>
                <TableRow>
                  <TableCell>Instance</TableCell>
                  <TableCell width={120} align="right">
                    State
                  </TableCell>
                  <TableCell width={40} />
                </TableRow>
              </TableHead>
              <TableBody>
                {r.instances.length === 0 ? (
                  <TableRow>
                    <TableCell colSpan={3} sx={{ color: "text.secondary" }}>
                      No instances.
                    </TableCell>
                  </TableRow>
                ) : (
                  r.instances.map((inst) => (
                    <TableRow key={inst.id}>
                      <TableCell
                        sx={{ fontFamily: "monospace", fontSize: "0.8rem" }}
                      >
                        {inst.display_name}
                      </TableCell>
                      <TableCell width={180} align="right">
                        <Box
                          sx={{
                            display: "flex",
                            gap: 0.5,
                            justifyContent: "flex-end",
                            alignItems: "center",
                          }}
                        >
                          {containerHealthcheck(r.def) && (
                            <HealthcheckIndicator
                              hc={containerHealthcheck(r.def)!}
                              lifecycle={inst.lifecycle}
                              instanceId={inst.id}
                              faults={r.faults}
                            />
                          )}
                          <Chip
                            label={inst.lifecycle.replace(/_/g, " ")}
                            color={lifecycleColor(inst.lifecycle)}
                            size="small"
                          />
                        </Box>
                      </TableCell>
                      <TableCell width={40} align="right" sx={{ px: 0.5 }}>
                        {(r.type === "deployment" || r.type === "job") && (
                          <IconActionButton
                            safety="read"
                            tooltip="View instance logs"
                            onClick={() =>
                              navigate(
                                `/apps/${appName}/logs?resource=${r.name}&instance=${inst.display_name}`,
                              )
                            }
                          >
                            <ArticleIcon sx={{ fontSize: 14 }} />
                          </IconActionButton>
                        )}
                      </TableCell>
                    </TableRow>
                  ))
                )}
              </TableBody>
            </Table>
          </TableContainer>
        </Box>
      ))}
      {snapshotDialog}
    </Stack>
  );
}

function ParamsSection({
  appName,
  params,
  status,
  onRefresh,
}: {
  appName: string;
  params: AppParam[];
  status: AppStatus;
  onRefresh: () => void;
}) {
  // Params cannot be mutated while the app has an operation in flight; the
  // backend rejects with operation_in_progress. Disable the edit/add
  // affordances and explain why, rather than letting users hit a server error.
  const operationInFlight = status === "installing" || status === "operating";
  const editsDisabled = operationInFlight;
  const { execute, loading, error, clearError } = useOiAction();
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [adding, setAdding] = useState(false);
  const [addName, setAddName] = useState("");
  const [addValue, setAddValue] = useState("");

  const startEdit = (p: AppParam) => {
    setEditing(p.name);
    setDraft(p.value ?? "");
    setShowPassword(false);
    clearError();
  };

  const cancel = () => {
    setEditing(null);
    setShowPassword(false);
  };

  const startAdd = () => {
    setAdding(true);
    setAddName("");
    setAddValue("");
    setEditing(null);
    clearError();
  };

  const cancelAdd = () => {
    setAdding(false);
    setAddName("");
    setAddValue("");
  };

  const saveAdd = async () => {
    if (!addName.trim()) return;
    try {
      await execute("/apps/params/set", {
        app: appName,
        name: addName.trim(),
        value: addValue,
      });
      setAdding(false);
      setAddName("");
      setAddValue("");
      onRefresh();
    } catch {
      // displayed via error
    }
  };

  const save = async () => {
    if (!editing) return;
    try {
      await execute("/apps/params/set", {
        app: appName,
        name: editing,
        value: draft,
      });
      setEditing(null);
      onRefresh();
    } catch {
      // displayed via error
    }
  };

  const unset = async (paramName: string) => {
    try {
      await execute("/apps/params/unset", { app: appName, name: paramName });
      onRefresh();
    } catch {
      // displayed via error
    }
  };

  const addRow = adding ? (
    <TableRow>
      <TableCell colSpan={2}>
        <TextField
          size="small"
          placeholder="param name"
          value={addName}
          onChange={(e) => setAddName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void saveAdd();
            if (e.key === "Escape") cancelAdd();
          }}
          autoFocus
          sx={{ width: 200 }}
          slotProps={{
            htmlInput: { style: { fontFamily: "monospace" } },
          }}
        />
      </TableCell>
      <TableCell>
        <TextField
          size="small"
          placeholder="value"
          value={addValue}
          onChange={(e) => setAddValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void saveAdd();
            if (e.key === "Escape") cancelAdd();
          }}
          fullWidth
          slotProps={{
            htmlInput: { style: { fontFamily: "monospace" } },
          }}
        />
      </TableCell>
      <TableCell align="right" sx={{ whiteSpace: "nowrap" }}>
        <IconActionButton
          safety="write"
          tooltip="Save"
          onClick={() => void saveAdd()}
          disabled={loading || !addName.trim()}
        >
          <CheckIcon fontSize="small" />
        </IconActionButton>
        <IconActionButton safety="read" tooltip="Cancel" onClick={cancelAdd}>
          <ClearIcon fontSize="small" />
        </IconActionButton>
      </TableCell>
    </TableRow>
  ) : null;

  const disabledBanner = operationInFlight ? (
    <Alert severity="info" sx={{ mb: 1 }}>
      Params are read-only while an operation is in progress for this app.
    </Alert>
  ) : null;

  if (params.length === 0 && !adding) {
    return (
      <Stack spacing={1}>
        {disabledBanner}
        {error && <OiErrorAlert error={error} />}
        <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
          <Typography
            sx={{
              color: "text.secondary",
            }}
          >
            No params.
          </Typography>
          <OutlinedActionButton
            safety="write"
            size="small"
            startIcon={<AddIcon fontSize="small" />}
            onClick={startAdd}
            disabled={editsDisabled}
          >
            Set param
          </OutlinedActionButton>
        </Box>
        {adding && (
          <TableContainer component={Paper} variant="outlined">
            <Table size="small">
              <TableBody>{addRow}</TableBody>
            </Table>
          </TableContainer>
        )}
      </Stack>
    );
  }

  return (
    <Stack spacing={1}>
      {disabledBanner}
      {error && <OiErrorAlert error={error} />}
      <Box sx={{ display: "flex", justifyContent: "flex-end" }}>
        {!adding && (
          <OutlinedActionButton
            safety="write"
            size="small"
            startIcon={<AddIcon fontSize="small" />}
            onClick={startAdd}
            disabled={editsDisabled}
          >
            Set param
          </OutlinedActionButton>
        )}
      </Box>
      <TableContainer component={Paper} variant="outlined">
        <Table size="small">
          <TableHead>
            <TableRow>
              <TableCell>Name</TableCell>
              <TableCell>Kind</TableCell>
              <TableCell>Value</TableCell>
              <TableCell width={96} />
            </TableRow>
          </TableHead>
          <TableBody>
            {params.map((p) =>
              editing === p.name ? (
                <TableRow key={p.name}>
                  <TableCell sx={{ fontFamily: "monospace" }}>
                    {p.name}
                  </TableCell>
                  <TableCell>
                    <Chip label={p.kind} size="small" variant="outlined" />
                  </TableCell>
                  <TableCell colSpan={2}>
                    <TextField
                      size="small"
                      fullWidth
                      value={draft}
                      onChange={(e) => setDraft(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && p.kind !== "multiline")
                          void save();
                        if (e.key === "Escape") cancel();
                      }}
                      autoFocus
                      multiline={p.kind === "multiline"}
                      minRows={p.kind === "multiline" ? 3 : undefined}
                      type={
                        p.kind === "multiline"
                          ? undefined
                          : showPassword
                            ? "text"
                            : paramFieldType(p.kind, p.secret)
                      }
                      error={
                        p.kind === "password" &&
                        draft.length > 0 &&
                        !isStrongPassword(draft)
                      }
                      helperText={
                        p.kind === "password" && draft.length > 0
                          ? isStrongPassword(draft)
                            ? (p.description ?? undefined)
                            : "Password is too weak"
                          : p.kind === "weak-password" && draft.length > 0
                            ? `Strength: ${passwordScore(draft)}/4${p.description ? ` — ${p.description}` : ""}`
                            : (p.description ?? undefined)
                      }
                      slotProps={{
                        input: {
                          endAdornment: (
                            <InputAdornment position="end">
                              {p.kind === "random" && (
                                <Tooltip title="Generate (32 bytes, hex)">
                                  <IconButton
                                    size="small"
                                    onClick={() =>
                                      setDraft(generateRandomHex())
                                    }
                                  >
                                    <CasinoIcon fontSize="small" />
                                  </IconButton>
                                </Tooltip>
                              )}
                              {(p.secret ||
                                p.kind === "password" ||
                                p.kind === "weak-password") && (
                                <Tooltip title={showPassword ? "Hide" : "Show"}>
                                  <IconButton
                                    size="small"
                                    onClick={() => setShowPassword((v) => !v)}
                                    edge="end"
                                  >
                                    {showPassword ? (
                                      <VisibilityOffIcon fontSize="small" />
                                    ) : (
                                      <VisibilityIcon fontSize="small" />
                                    )}
                                  </IconButton>
                                </Tooltip>
                              )}
                              <Tooltip title="Save">
                                <span>
                                  <IconButton
                                    size="small"
                                    onClick={() => void save()}
                                    disabled={loading}
                                  >
                                    <CheckIcon fontSize="small" />
                                  </IconButton>
                                </span>
                              </Tooltip>
                              <Tooltip title="Cancel">
                                <IconButton size="small" onClick={cancel}>
                                  <ClearIcon fontSize="small" />
                                </IconButton>
                              </Tooltip>
                            </InputAdornment>
                          ),
                        },

                        htmlInput: { style: { fontFamily: "monospace" } },
                      }}
                    />
                  </TableCell>
                </TableRow>
              ) : (
                <TableRow key={p.name}>
                  <TableCell>
                    <Box sx={{ fontFamily: "monospace" }}>
                      {p.name}
                      {p.required && (
                        <Typography
                          component="span"
                          color="error"
                          sx={{ ml: 0.5 }}
                        >
                          *
                        </Typography>
                      )}
                    </Box>
                    {p.description && (
                      <Typography
                        variant="caption"
                        sx={{
                          color: "text.secondary",
                        }}
                      >
                        {p.description}
                      </Typography>
                    )}
                  </TableCell>
                  <TableCell>
                    <Chip label={p.kind} size="small" variant="outlined" />
                  </TableCell>
                  <TableCell sx={{ fontFamily: "monospace" }}>
                    {(() => {
                      const isMasked =
                        p.secret ||
                        p.kind === "password" ||
                        p.kind === "weak-password";
                      if (p.secret && p.is_set) {
                        return "••••••••";
                      }
                      if (p.value != null) {
                        return isMasked ? "••••••••" : p.value;
                      }
                      if (p.default_value != null) {
                        return (
                          <Box component="span" sx={{ color: "text.disabled" }}>
                            {isMasked ? "••••••••" : p.default_value}
                            <Typography
                              component="span"
                              variant="caption"
                              sx={{ ml: 0.5 }}
                            >
                              (default)
                            </Typography>
                          </Box>
                        );
                      }
                      return (
                        <Box component="span" sx={{ color: "text.disabled" }}>
                          —
                        </Box>
                      );
                    })()}
                  </TableCell>
                  <TableCell align="right" sx={{ whiteSpace: "nowrap" }}>
                    <IconActionButton
                      safety="write"
                      tooltip={p.value == null && !p.is_set ? "Set" : "Edit"}
                      onClick={() => startEdit(p)}
                      disabled={loading || editsDisabled}
                    >
                      <EditIcon fontSize="small" />
                    </IconActionButton>
                    {p.value != null && !p.required && (
                      <IconActionButton
                        safety="write"
                        tooltip="Unset"
                        onClick={() => void unset(p.name)}
                        disabled={loading || editsDisabled}
                      >
                        <DeleteOutlineIcon fontSize="small" />
                      </IconActionButton>
                    )}
                    {p.value != null &&
                      p.required &&
                      p.default_value != null && (
                        <IconActionButton
                          safety="write"
                          tooltip="Reset to default"
                          onClick={() => void unset(p.name)}
                          disabled={loading || editsDisabled}
                        >
                          <RestoreIcon fontSize="small" />
                        </IconActionButton>
                      )}
                  </TableCell>
                </TableRow>
              ),
            )}
            {addRow}
          </TableBody>
        </Table>
      </TableContainer>
    </Stack>
  );
}

function paramFieldType(kind: string, secret?: boolean): string {
  if (secret || kind === "password" || kind === "weak-password")
    return "password";
  if (kind === "email") return "email";
  return "text";
}

// l[impl action.install.requirements.kind-random]
// Default `random` generator output: 32 bytes, lowercase hex (64 chars).
function generateRandomHex(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function ActionInvokeDialog({
  appName,
  action,
  open,
  onClose,
  onSuccess,
}: {
  appName: string;
  action: AppAction;
  open: boolean;
  onClose: () => void;
  onSuccess: () => void;
}) {
  const { execute, loading, error, clearError } = useOiAction();
  const [values, setValues] = useState<Record<string, string>>(() =>
    Object.fromEntries(
      Object.entries(action.params).map(
        ([k, def]: [string, InstallRequirement]) => [
          k,
          def.default_value ?? "",
        ],
      ),
    ),
  );
  const [showPasswords, setShowPasswords] = useState<Record<string, boolean>>(
    {},
  );

  const toggleShow = (key: string) =>
    setShowPasswords((s) => ({ ...s, [key]: !s[key] }));

  const handleClose = () => {
    clearError();
    onClose();
  };

  const handleSubmit = async () => {
    const method =
      action.kind === "install"
        ? "/apps/install/invoke"
        : "/apps/action/invoke";
    const params =
      action.kind === "install"
        ? { app: appName, params: values }
        : { app: appName, name: action.name, params: values };
    try {
      await execute(method, params);
      onSuccess();
      handleClose();
    } catch {
      // displayed via error
    }
  };

  const paramEntries = Object.entries(action.params) as [
    string,
    InstallRequirement,
  ][];

  const hasWeakPassword = paramEntries.some(
    ([key, def]) =>
      def.kind === "password" &&
      values[key] != null &&
      !isStrongPassword(values[key]),
  );

  // l[impl action.params.volume]
  // The dialog opens on demand, so the extra round-trip when the action has
  // no volume params is acceptable; the alternative is a conditional hook
  // (which useOiQuery doesn't support).
  const { data: siteVolumes } = useOiQuery<SiteVolume[]>(
    "/volumes/site/list",
    {},
  );

  return (
    <Dialog open={open} onClose={handleClose} maxWidth="sm" fullWidth>
      <DialogTitle sx={{ fontFamily: "monospace", pb: 1 }}>
        {action.kind === "install" ? "Install" : `Run: ${action.name}`}
      </DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 0.5 }}>
          {error && <OiErrorAlert error={error} />}
          {paramEntries.length === 0 ? (
            <Typography
              variant="body2"
              sx={{
                color: "text.secondary",
              }}
            >
              No params required.
            </Typography>
          ) : (
            paramEntries.map(([key, def]) => {
              if (def.kind === "volume") {
                return (
                  <FormControl
                    key={key}
                    size="small"
                    required={def.required}
                    fullWidth
                  >
                    <InputLabel id={`volume-${key}-label`}>{key}</InputLabel>
                    <Select
                      labelId={`volume-${key}-label`}
                      label={key}
                      value={values[key] ?? ""}
                      onChange={(e) =>
                        setValues((v) => ({
                          ...v,
                          [key]: e.target.value as string,
                        }))
                      }
                    >
                      {(siteVolumes ?? []).map((v) => (
                        <MenuItem key={v.name} value={v.name}>
                          <Box
                            sx={{ fontFamily: "monospace", display: "inline" }}
                          >
                            {v.name}
                          </Box>
                          <Box
                            component="span"
                            sx={{ ml: 1, color: "text.secondary" }}
                          >
                            {v.kind}
                          </Box>
                        </MenuItem>
                      ))}
                    </Select>
                    {def.description && (
                      <FormHelperText>{def.description}</FormHelperText>
                    )}
                  </FormControl>
                );
              }
              const isPassword =
                def.kind === "password" || def.kind === "weak-password";
              const isRandom = def.kind === "random";
              const isMultiline = def.kind === "multiline";
              const val = values[key] ?? "";
              const weak =
                def.kind === "password" &&
                val.length > 0 &&
                !isStrongPassword(val);
              const helperText =
                def.kind === "password" && val.length > 0
                  ? weak
                    ? "Password is too weak"
                    : (def.description ?? undefined)
                  : def.kind === "weak-password" && val.length > 0
                    ? `Strength: ${passwordScore(val)}/4${def.description ? ` — ${def.description}` : ""}`
                    : (def.description ?? undefined);
              return (
                <TextField
                  key={key}
                  label={key}
                  size="small"
                  value={val}
                  onChange={(e) =>
                    setValues((v) => ({ ...v, [key]: e.target.value }))
                  }
                  helperText={helperText}
                  error={weak}
                  multiline={isMultiline}
                  minRows={isMultiline ? 3 : undefined}
                  type={
                    isMultiline
                      ? undefined
                      : showPasswords[key]
                        ? "text"
                        : paramFieldType(def.kind)
                  }
                  required={def.required}
                  slotProps={{
                    input:
                      isPassword || isRandom
                        ? {
                            endAdornment: (
                              <InputAdornment position="end">
                                {isRandom && (
                                  <Tooltip title="Generate (32 bytes, hex)">
                                    <IconButton
                                      size="small"
                                      onClick={() =>
                                        setValues((v) => ({
                                          ...v,
                                          [key]: generateRandomHex(),
                                        }))
                                      }
                                    >
                                      <CasinoIcon fontSize="small" />
                                    </IconButton>
                                  </Tooltip>
                                )}
                                {isPassword && (
                                  <Tooltip
                                    title={showPasswords[key] ? "Hide" : "Show"}
                                  >
                                    <IconButton
                                      size="small"
                                      onClick={() => toggleShow(key)}
                                      edge="end"
                                    >
                                      {showPasswords[key] ? (
                                        <VisibilityOffIcon fontSize="small" />
                                      ) : (
                                        <VisibilityIcon fontSize="small" />
                                      )}
                                    </IconButton>
                                  </Tooltip>
                                )}
                              </InputAdornment>
                            ),
                          }
                        : undefined,

                    htmlInput: { style: { fontFamily: "monospace" } },
                  }}
                />
              );
            })
          )}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={handleClose} disabled={loading}>
          Cancel
        </Button>
        <SolidActionButton
          safety="write"
          onClick={handleSubmit}
          disabled={loading || hasWeakPassword}
        >
          {loading ? "Running…" : action.kind === "install" ? "Install" : "Run"}
        </SolidActionButton>
      </DialogActions>
    </Dialog>
  );
}

function InstallSection({
  appName,
  installAction,
  hasScriptError,
  faults,
  onRefresh,
}: {
  appName: string;
  installAction: AppAction | undefined;
  hasScriptError: boolean;
  faults: FaultRecord[];
  onRefresh: () => void;
}) {
  const { execute, loading } = useOiAction();
  const [dialogOpen, setDialogOpen] = useState(false);

  const hasParams =
    installAction && Object.keys(installAction.params).length > 0;
  // Surface the most-recent operation_failed fault so operators who land here
  // after a failed install (or just after uninstall) can see what went wrong
  // and jump straight to the logs, without having to hunt for the Logs button.
  const operationFailures = faults.filter((f) => f.kind === "operation_failed");

  const handleInstall = async () => {
    if (hasParams) {
      setDialogOpen(true);
    } else {
      await execute("/apps/install/invoke", { app: appName, params: {} });
      onRefresh();
    }
  };

  return (
    <>
      <Box
        sx={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: 2,
          py: 6,
        }}
      >
        <Typography
          sx={{
            color: "text.secondary",
          }}
        >
          This app has not been installed yet.
        </Typography>
        <SolidActionButton
          safety="write"
          size="large"
          onClick={() => void handleInstall()}
          disabled={loading || hasScriptError}
        >
          {loading ? "Installing…" : "Install"}
        </SolidActionButton>
        {installAction && (
          <Typography
            variant="caption"
            sx={{ color: "text.secondary", textAlign: "center", maxWidth: 480 }}
          >
            Runs the app's{" "}
            <Box component="code" sx={{ fontFamily: "monospace" }}>
              on_install
            </Box>{" "}
            action
            {installAction.description ? ` — ${installAction.description}` : ""}
            .
          </Typography>
        )}
        {operationFailures.length > 0 && (
          <Alert severity="error" sx={{ width: "100%", mt: 2 }}>
            <Typography variant="subtitle2" gutterBottom>
              The last install attempt failed:
            </Typography>
            {operationFailures.map((f) => (
              <Typography
                key={f.id}
                variant="body2"
                sx={{ fontFamily: "monospace", whiteSpace: "pre-wrap" }}
              >
                {f.description}
              </Typography>
            ))}
          </Alert>
        )}
        <Button
          size="small"
          startIcon={<ArticleIcon />}
          component={Link}
          to={`/apps/${appName}/logs`}
        >
          View logs from previous runs
        </Button>
      </Box>
      {installAction && (
        <ActionInvokeDialog
          appName={appName}
          action={installAction}
          open={dialogOpen}
          onClose={() => setDialogOpen(false)}
          onSuccess={onRefresh}
        />
      )}
    </>
  );
}

type ActionScheduleRow = ActionSchedule & { action: string };

function collectActionSchedules(actions: AppAction[]): ActionScheduleRow[] {
  const rows: ActionScheduleRow[] = [];
  for (const a of actions) {
    for (const s of a.schedules) {
      rows.push({ action: a.name, ...s });
    }
  }
  return rows;
}

function SchedulesSection({ actions }: { actions: AppAction[] }) {
  const rows = useMemo(() => collectActionSchedules(actions), [actions]);
  if (rows.length === 0) return null;
  return (
    <TableContainer component={Paper} variant="outlined">
      <Table size="small">
        <TableHead>
          <TableRow>
            <TableCell>Action</TableCell>
            <TableCell>Schedule</TableCell>
            <TableCell>Last fire</TableCell>
            <TableCell>Next fire</TableCell>
          </TableRow>
        </TableHead>
        <TableBody>
          {rows.map((r) => (
            <TableRow key={`${r.action}::${r.cronexpr}`}>
              <TableCell sx={{ fontFamily: "monospace" }}>{r.action}</TableCell>
              <TableCell sx={{ fontFamily: "monospace" }}>
                {r.cronexpr}
              </TableCell>
              <TableCell
                sx={{ color: r.last_fired_at ? undefined : "text.disabled" }}
              >
                {r.last_fired_at
                  ? new Date(r.last_fired_at).toLocaleString()
                  : "never"}
              </TableCell>
              <TableCell
                sx={{ color: r.next_fire_at ? undefined : "text.disabled" }}
              >
                {r.next_fire_at
                  ? new Date(r.next_fire_at).toLocaleString()
                  : "—"}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </TableContainer>
  );
}

function ActionsSection({
  appName,
  actions,
  status,
  hasScriptError,
  operatingAction,
  onRefresh,
}: {
  appName: string;
  actions: AppAction[];
  status: AppStatus;
  hasScriptError: boolean;
  operatingAction?: string;
  onRefresh: () => void;
}) {
  const [invoking, setInvoking] = useState<AppAction | null>(null);
  const [openingShell, setOpeningShell] = useState<AppAction | null>(null);
  const { openShell } = useSessionContext();

  const canInvoke =
    !hasScriptError &&
    status !== "not_installed" &&
    status !== "installing" &&
    status !== "uninstalling" &&
    status !== "deregistering" &&
    status !== "operating";

  if (actions.length === 0)
    return (
      <Typography
        sx={{
          color: "text.secondary",
        }}
      >
        No actions.
      </Typography>
    );

  return (
    <>
      <TableContainer component={Paper} variant="outlined">
        <Table size="small">
          <TableHead>
            <TableRow>
              <TableCell>Name</TableCell>
              <TableCell>Kind</TableCell>
              <TableCell>Description</TableCell>
              <TableCell />
            </TableRow>
          </TableHead>
          <TableBody>
            {/* The install action is invoked from the dedicated Install
                button in InstallSection, and the App detail Actions
                section only renders for already-installed apps anyway —
                so hide the install row, which would otherwise sit there
                permanently un-invokable. */}
            {actions
              .filter((a) => a.kind !== "install")
              .map((a) => {
                const isInvokable =
                  a.kind !== "shell" && a.kind !== "lifecycle";
                const isRunning = a.name === operatingAction;
                const canRun = isInvokable && canInvoke;
                return (
                  <TableRow key={a.name}>
                    <TableCell sx={{ fontFamily: "monospace" }}>
                      <Box
                        sx={{
                          display: "flex",
                          alignItems: "center",
                          gap: 0.5,
                        }}
                      >
                        <span>{a.name}</span>
                        {a.schedules.length > 0 && (
                          <Tooltip
                            title={
                              <span style={{ whiteSpace: "pre-line" }}>
                                {a.schedules
                                  .map((s) => `schedule: ${s.cronexpr}`)
                                  .join("\n")}
                              </span>
                            }
                          >
                            <Chip
                              label={
                                a.schedules.length === 1
                                  ? "scheduled"
                                  : `scheduled ×${a.schedules.length}`
                              }
                              size="small"
                              variant="outlined"
                              color="info"
                              sx={{
                                fontSize: "0.65rem",
                                height: 18,
                                "& .MuiChip-label": { px: 0.75 },
                              }}
                            />
                          </Tooltip>
                        )}
                      </Box>
                    </TableCell>
                    <TableCell>
                      <Chip label={a.kind} size="small" variant="outlined" />
                    </TableCell>
                    <TableCell sx={{ color: "text.secondary" }}>
                      {a.description}
                    </TableCell>
                    <TableCell align="right">
                      {/* w[shells.ui] */}
                      {a.kind === "shell" ? (
                        <OutlinedActionButton
                          safety="write"
                          size="small"
                          onClick={() => {
                            if (Object.keys(a.params).length > 0) {
                              setOpeningShell(a);
                            } else {
                              openShell(appName, a.name, {});
                            }
                          }}
                          disabled={!canInvoke}
                        >
                          shell
                        </OutlinedActionButton>
                      ) : (
                        isInvokable &&
                        (isRunning ? (
                          <Button
                            size="small"
                            variant="outlined"
                            disabled
                            startIcon={<CircularProgress size={12} />}
                          >
                            Running…
                          </Button>
                        ) : (
                          <OutlinedActionButton
                            safety="write"
                            size="small"
                            onClick={() => setInvoking(a)}
                            disabled={!canRun}
                          >
                            Run
                          </OutlinedActionButton>
                        ))
                      )}
                    </TableCell>
                  </TableRow>
                );
              })}
          </TableBody>
        </Table>
      </TableContainer>
      {invoking && (
        <ActionInvokeDialog
          key={invoking.name}
          appName={appName}
          action={invoking}
          open={true}
          onClose={() => setInvoking(null)}
          onSuccess={onRefresh}
        />
      )}
      {openingShell && (
        <ShellOpenDialog
          key={openingShell.name}
          action={openingShell}
          open={true}
          onClose={() => setOpeningShell(null)}
          onOpen={(params) => {
            openShell(appName, openingShell.name, params);
            setOpeningShell(null);
          }}
        />
      )}
    </>
  );
}

function ShellOpenDialog({
  action,
  open,
  onClose,
  onOpen,
}: {
  action: AppAction;
  open: boolean;
  onClose: () => void;
  onOpen: (params: Record<string, string>) => void;
}) {
  const [values, setValues] = useState<Record<string, string>>(() =>
    Object.fromEntries(
      Object.entries(action.params).map(
        ([k, def]: [string, InstallRequirement]) => [
          k,
          def.default_value ?? "",
        ],
      ),
    ),
  );
  const [showPasswords, setShowPasswords] = useState<Record<string, boolean>>(
    {},
  );

  const toggleShow = (key: string) =>
    setShowPasswords((s) => ({ ...s, [key]: !s[key] }));

  const paramEntries = Object.entries(action.params) as [
    string,
    InstallRequirement,
  ][];
  const hasWeakPassword = paramEntries.some(
    ([key, def]) =>
      def.kind === "password" &&
      values[key] != null &&
      !isStrongPassword(values[key]),
  );

  return (
    <Dialog open={open} onClose={onClose} maxWidth="sm" fullWidth>
      <DialogTitle sx={{ fontFamily: "monospace", pb: 1 }}>
        Open shell: {action.name}
      </DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 0.5 }}>
          {paramEntries.map(([key, def]) => {
            const isPassword =
              def.kind === "password" || def.kind === "weak-password";
            const isRandom = def.kind === "random";
            const isMultiline = def.kind === "multiline";
            const val = values[key] ?? "";
            const weak =
              def.kind === "password" &&
              val.length > 0 &&
              !isStrongPassword(val);
            const helperText =
              def.kind === "password" && val.length > 0
                ? weak
                  ? "Password is too weak"
                  : (def.description ?? undefined)
                : def.kind === "weak-password" && val.length > 0
                  ? `Strength: ${passwordScore(val)}/4${def.description ? ` — ${def.description}` : ""}`
                  : (def.description ?? undefined);
            return (
              <TextField
                key={key}
                label={key}
                size="small"
                value={val}
                onChange={(e) =>
                  setValues((v) => ({ ...v, [key]: e.target.value }))
                }
                helperText={helperText}
                error={weak}
                multiline={isMultiline}
                minRows={isMultiline ? 3 : undefined}
                type={
                  isMultiline
                    ? undefined
                    : showPasswords[key]
                      ? "text"
                      : paramFieldType(def.kind)
                }
                required={def.required}
                slotProps={{
                  input:
                    isPassword || isRandom
                      ? {
                          endAdornment: (
                            <InputAdornment position="end">
                              {isRandom && (
                                <Tooltip title="Generate (32 bytes, hex)">
                                  <IconButton
                                    size="small"
                                    onClick={() =>
                                      setValues((v) => ({
                                        ...v,
                                        [key]: generateRandomHex(),
                                      }))
                                    }
                                  >
                                    <CasinoIcon fontSize="small" />
                                  </IconButton>
                                </Tooltip>
                              )}
                              {isPassword && (
                                <Tooltip
                                  title={showPasswords[key] ? "Hide" : "Show"}
                                >
                                  <IconButton
                                    size="small"
                                    onClick={() => toggleShow(key)}
                                    edge="end"
                                  >
                                    {showPasswords[key] ? (
                                      <VisibilityOffIcon fontSize="small" />
                                    ) : (
                                      <VisibilityIcon fontSize="small" />
                                    )}
                                  </IconButton>
                                </Tooltip>
                              )}
                            </InputAdornment>
                          ),
                        }
                      : undefined,

                  htmlInput: { style: { fontFamily: "monospace" } },
                }}
              />
            );
          })}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose}>Cancel</Button>
        <SolidActionButton
          safety="write"
          onClick={() => onOpen(values)}
          disabled={hasWeakPassword}
        >
          shell
        </SolidActionButton>
      </DialogActions>
    </Dialog>
  );
}

function humanBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GiB`;
}

// w[impl routes.images.discover]
function DiscoverSummary({ result }: { result: DiscoverResponse }) {
  const problems = result.per_handler.filter(
    (h) => h.error !== null || h.skipped_reason !== null,
  );
  if (problems.length === 0) return null;
  return (
    <Alert severity="info" sx={{ mb: 1 }}>
      <Stack spacing={0.5}>
        <Typography variant="body2">
          Some handlers couldn't be probed cleanly; their images (if any) aren't
          included in the results.
        </Typography>
        {problems.map((h: HandlerProbe) => (
          <Typography
            key={`${h.kind}:${h.name}`}
            variant="caption"
            sx={{ fontFamily: "monospace" }}
          >
            {h.kind}/{h.name}: {h.error ?? h.skipped_reason}
          </Typography>
        ))}
      </Stack>
    </Alert>
  );
}

// w[impl routes.images.app-detail]
function AppImagesSection({
  appName,
  resources,
  onRefresh,
}: {
  appName: string;
  resources: AppResource[];
  onRefresh: () => void;
}) {
  const { data: imagesData, refetch: refetchImages } = useOiQuery<{
    images: ImageSummary[];
  }>("/images/list", {});
  const { data: pinsData, refetch: refetchPins } = useOiQuery<{
    pins: ImagePin[];
  }>("/images/pins/list", { app: appName });
  const {
    execute,
    loading: mutating,
    error: mutateError,
    clearError,
  } = useOiAction();

  const [removing, setRemoving] = useState<ImageSummary | null>(null);
  const [clearAllOpen, setClearAllOpen] = useState(false);
  const [discoverResult, setDiscoverResult] = useState<DiscoverResponse | null>(
    null,
  );
  const [discovering, setDiscovering] = useState(false);
  const [warmingAll, setWarmingAll] = useState(false);

  // Image refs declared by this app's container resources.
  const declaredImages = useMemo(() => {
    const refs = new Set<string>();
    for (const r of resources) {
      const img =
        r.def?.kind === "deployment" || r.def?.kind === "job"
          ? r.def.container.image
          : null;
      if (img) refs.add(img);
    }
    return refs;
  }, [resources]);

  const pins = pinsData?.pins ?? [];
  const pinnedRefs = useMemo(
    () => new Set(pins.map((p) => p.reference)),
    [pins],
  );

  // Show images either (a) referenced by this app's resources and in-use,
  // or (b) pinned by this app.
  const rows = useMemo<ImageSummary[]>(() => {
    const all = imagesData?.images ?? [];
    return all.filter((img) => {
      const refs = [...img.tags, ...img.digests.map((d) => d.reference)];
      const hitsDeclared =
        img.in_use && refs.some((r) => declaredImages.has(r));
      const hitsPin =
        img.pinned_by.includes(appName) || refs.some((r) => pinnedRefs.has(r));
      return hitsDeclared || hitsPin;
    });
  }, [imagesData, declaredImages, pinnedRefs, appName]);

  const refreshAll = () => {
    onRefresh();
    refetchImages();
    refetchPins();
  };

  const submitRemove = async () => {
    if (!removing) return;
    try {
      await execute("/images/remove", {
        reference: primaryReference(removing),
      });
      setRemoving(null);
      refreshAll();
    } catch {
      /* surfaced via mutateError */
    }
  };

  const submitClearAll = async () => {
    try {
      await execute("/images/pins/clear", { app: appName });
      setClearAllOpen(false);
      refreshAll();
    } catch {
      /* surfaced via mutateError */
    }
  };

  // w[impl routes.images.discover]
  const runDiscover = async () => {
    clearError();
    setDiscovering(true);
    try {
      const result = (await execute("/apps/images/discover", {
        app: appName,
        lenient: true,
      })) as DiscoverResponse;
      setDiscoverResult(result);
    } catch {
      /* surfaced via mutateError */
    } finally {
      setDiscovering(false);
    }
  };

  // Discovered references not already in-use (any reference) and not pinned.
  const discoveredExtras = useMemo<string[]>(() => {
    if (!discoverResult) return [];
    const present = new Set<string>();
    for (const img of imagesData?.images ?? []) {
      if (img.in_use) {
        for (const t of img.tags) present.add(t);
        for (const d of img.digests) present.add(d.reference);
      }
    }
    for (const p of pins) present.add(p.reference);
    return discoverResult.all_images.filter((r) => !present.has(r));
  }, [discoverResult, imagesData, pins]);

  const warmReference = async (reference: string) => {
    try {
      await execute("/images/pull", { reference, app: appName });
      refreshAll();
    } catch {
      /* surfaced via mutateError */
    }
  };

  // w[impl routes.images.discover]
  const warmAllDiscovered = async () => {
    if (discoveredExtras.length === 0) return;
    setWarmingAll(true);
    for (const ref of discoveredExtras) {
      try {
        await execute("/images/pull", { reference: ref, app: appName });
      } catch {
        break;
      }
    }
    setWarmingAll(false);
    refreshAll();
  };

  const hasAnything =
    rows.length > 0 || pins.length > 0 || discoverResult !== null;

  if (!hasAnything && !discovering) {
    // Show just the discover button so operators have a way to find
    // handler-only images even when no static/pinned images are present.
    return (
      <>
        <Box
          sx={{
            display: "flex",
            alignItems: "center",
            mb: 1,
            gap: 1,
          }}
        >
          <Typography variant="h6" sx={{ flexGrow: 1 }}>
            Images
          </Typography>
          <Tooltip title="Run handler probe to discover images that actions might pull">
            <span>
              <Button
                size="small"
                variant="outlined"
                onClick={runDiscover}
                disabled={mutating || discovering}
              >
                Discover from handlers
              </Button>
            </span>
          </Tooltip>
        </Box>
        {mutateError && <OiErrorAlert error={mutateError} />}
      </>
    );
  }

  return (
    <>
      <Box
        sx={{
          display: "flex",
          alignItems: "center",
          mb: 1,
          gap: 1,
          flexWrap: "wrap",
        }}
      >
        <Typography variant="h6" sx={{ flexGrow: 1 }}>
          Images
        </Typography>
        {/* w[impl routes.images.discover] */}
        {discoveredExtras.length > 0 && (
          <SolidActionButton
            safety="write"
            tooltip={`Warm ${discoveredExtras.length} discovered image${discoveredExtras.length === 1 ? "" : "s"}`}
            size="small"
            onClick={warmAllDiscovered}
            disabled={mutating || warmingAll}
          >
            {warmingAll ? "Warming…" : "Warm all discovered"}
          </SolidActionButton>
        )}
        <OutlinedActionButton
          safety="read"
          tooltip="Run handler probe to discover images that actions might pull"
          size="small"
          onClick={runDiscover}
          disabled={mutating || discovering}
        >
          {discovering ? "Discovering…" : "Discover from handlers"}
        </OutlinedActionButton>
        {pins.length > 0 && (
          <OutlinedActionButton
            safety="write"
            tooltip={`Clear all ${pins.length} pin${pins.length === 1 ? "" : "s"} for this app`}
            size="small"
            onClick={() => {
              clearError();
              setClearAllOpen(true);
            }}
            disabled={mutating}
          >
            Clear all pins
          </OutlinedActionButton>
        )}
      </Box>
      {mutateError && <OiErrorAlert error={mutateError} />}
      {discoverResult && <DiscoverSummary result={discoverResult} />}
      <TableContainer component={Paper} variant="outlined">
        <Table size="small">
          <TableHead>
            <TableRow>
              <TableCell>Reference</TableCell>
              <TableCell>Size</TableCell>
              <TableCell>State</TableCell>
              <TableCell width={80} align="right" />
            </TableRow>
          </TableHead>
          <TableBody>
            {rows.map((img) => (
              <TableRow key={img.image_id} hover>
                <TableCell>
                  <ImageReferencesCell image={img} />
                </TableCell>
                <TableCell>{humanBytes(img.size_bytes)}</TableCell>
                <TableCell>
                  <Stack direction="row" spacing={0.5}>
                    {img.in_use && (
                      <Chip label="in use" size="small" color="success" />
                    )}
                    {img.pinned_by.includes(appName) && (
                      <Chip
                        label="pinned"
                        size="small"
                        color="primary"
                        variant="outlined"
                      />
                    )}
                  </Stack>
                </TableCell>
                <TableCell align="right">
                  <IconActionButton
                    safety="dangerous"
                    tooltip={
                      img.in_use ? "Cannot remove: image is in use" : "Remove"
                    }
                    onClick={() => {
                      clearError();
                      setRemoving(img);
                    }}
                    disabled={img.in_use}
                  >
                    <DeleteOutlineIcon fontSize="small" />
                  </IconActionButton>
                </TableCell>
              </TableRow>
            ))}
            {/* w[impl routes.images.discover] */}
            {discoveredExtras.map((ref) => (
              <TableRow key={`discovered::${ref}`} hover>
                <TableCell>
                  <Typography variant="body2" sx={{ fontFamily: "monospace" }}>
                    {ref}
                  </Typography>
                </TableCell>
                <TableCell>
                  <Typography variant="caption" sx={{ color: "text.disabled" }}>
                    not present
                  </Typography>
                </TableCell>
                <TableCell>
                  <Chip
                    label="potentially used"
                    size="small"
                    color="warning"
                    variant="outlined"
                  />
                </TableCell>
                <TableCell align="right">
                  <OutlinedActionButton
                    safety="write"
                    tooltip="Pull and pin to this app"
                    size="small"
                    onClick={() => warmReference(ref)}
                    disabled={mutating}
                  >
                    Warm
                  </OutlinedActionButton>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </TableContainer>

      {/* w[impl routes.images.confirm] */}
      <Dialog
        open={removing !== null}
        onClose={() => setRemoving(null)}
        fullWidth
        maxWidth="sm"
      >
        <DialogTitle>Remove image</DialogTitle>
        <DialogContent>
          {removing && (
            <Stack spacing={2} sx={{ mt: 1 }}>
              <Typography>
                Remove <code>{primaryReference(removing)}</code> from local
                storage? This will fail if a running container is using the
                image.
              </Typography>
              {mutateError && <OiErrorAlert error={mutateError} />}
            </Stack>
          )}
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setRemoving(null)} disabled={mutating}>
            Cancel
          </Button>
          <SolidActionButton
            safety="dangerous"
            onClick={submitRemove}
            disabled={mutating}
          >
            Remove
          </SolidActionButton>
        </DialogActions>
      </Dialog>

      <Dialog
        open={clearAllOpen}
        onClose={() => setClearAllOpen(false)}
        fullWidth
        maxWidth="sm"
      >
        <DialogTitle>Clear image pins</DialogTitle>
        <DialogContent>
          <Stack spacing={2} sx={{ mt: 1 }}>
            <Typography>
              Clear all {pins.length} image pin{pins.length === 1 ? "" : "s"}{" "}
              held by <strong>{appName}</strong>? Pinned images stay in local
              storage but are no longer protected from autonomous GC.
            </Typography>
            {mutateError && <OiErrorAlert error={mutateError} />}
          </Stack>
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setClearAllOpen(false)} disabled={mutating}>
            Cancel
          </Button>
          <SolidActionButton
            safety="write"
            onClick={submitClearAll}
            disabled={mutating}
          >
            Clear all pins
          </SolidActionButton>
        </DialogActions>
      </Dialog>
    </>
  );
}

function ClearFaultsButton({
  appName,
  status,
  onCleared,
}: {
  appName: string;
  status: AppStatus;
  onCleared: () => void;
}) {
  // i[impl fault.clear-app]
  // Clearing faults from a not-installed app is write-level; from any other
  // phase it's danger-level because the operator may be silencing live signal
  // about a running workload's problems.
  const tier = status === "not_installed" ? "write" : "dangerous";
  const { execute, loading, error, clearError } = useOiAction();
  const [confirming, setConfirming] = useState(false);
  return (
    <>
      <OutlinedActionButton
        safety={tier}
        tooltip="Clear all active faults for this app"
        size="small"
        disabled={loading}
        onClick={() => setConfirming(true)}
      >
        Clear all
      </OutlinedActionButton>
      <Dialog
        open={confirming}
        onClose={() => {
          setConfirming(false);
          clearError();
        }}
      >
        <DialogTitle>Clear all faults?</DialogTitle>
        <DialogContent>
          <DialogContentText>
            This clears every active fault for <strong>{appName}</strong>.
            Faults derived from observable conditions (image pull failures,
            healthcheck failures, etc.) will be re-filed on the next
            reconciliation tick if the underlying problem still exists.
            {tier === "dangerous" && (
              <>
                {" "}
                This is a danger-level action because the app is currently{" "}
                <strong>{status}</strong> — clearing live signal can obscure
                problems that operators need to see.
              </>
            )}
          </DialogContentText>
          {error && (
            <Alert severity="error" sx={{ mt: 2 }}>
              {error.message}
            </Alert>
          )}
        </DialogContent>
        <DialogActions>
          <Button
            onClick={() => {
              setConfirming(false);
              clearError();
            }}
          >
            Cancel
          </Button>
          <Button
            color={tier === "dangerous" ? "error" : "primary"}
            variant="contained"
            disabled={loading}
            onClick={async () => {
              try {
                await execute("/faults/clear", { app: appName });
                setConfirming(false);
                onCleared();
              } catch {
                // error state is set by useOiAction; dialog stays open
              }
            }}
          >
            Clear faults
          </Button>
        </DialogActions>
      </Dialog>
    </>
  );
}

function Section({
  title,
  action,
  children,
}: {
  title: string;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <Box>
      <Box
        sx={{
          mb: 1,
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 1,
        }}
      >
        <Typography variant="h6">{title}</Typography>
        {action}
      </Box>
      {children}
    </Box>
  );
}

function AppRemovalDialog({
  appName,
  kind,
  open,
  onClose,
  onSuccess,
}: {
  appName: string;
  kind: "uninstall" | "deregister";
  open: boolean;
  onClose: () => void;
  onSuccess: () => void;
}) {
  const { execute, loading, error, clearError } = useOiAction();

  const handleConfirm = async () => {
    try {
      const method = kind === "uninstall" ? "/apps/uninstall" : "/apps/remove";
      await execute(method, { app: appName });
      onSuccess();
    } catch {
      // displayed via error
    }
  };

  const handleClose = () => {
    clearError();
    onClose();
  };

  return (
    <Dialog open={open} onClose={handleClose} maxWidth="xs" fullWidth>
      <DialogTitle>
        {kind === "uninstall" ? "Uninstall app" : "Deregister app"}
      </DialogTitle>
      <DialogContent>
        {error && <OiErrorAlert error={error} />}
        <Typography>
          {kind === "uninstall" ? (
            <>
              Uninstall <strong>{appName}</strong>? This will tear down all its
              resources. The app will remain registered and can be reinstalled.
            </>
          ) : (
            <>
              Remove <strong>{appName}</strong> from Seedling entirely? This
              cannot be undone.
            </>
          )}
        </Typography>
      </DialogContent>
      <DialogActions>
        <Button onClick={handleClose} disabled={loading}>
          Cancel
        </Button>
        <SolidActionButton
          safety="dangerous"
          onClick={handleConfirm}
          disabled={loading}
        >
          {loading
            ? kind === "uninstall"
              ? "Uninstalling…"
              : "Removing…"
            : kind === "uninstall"
              ? "Uninstall"
              : "Deregister"}
        </SolidActionButton>
      </DialogActions>
    </Dialog>
  );
}

const APP_DETAIL_EVENTS: Set<string> = new Set([
  "AppUpdated",
  "AppPhaseChanged",
  "OperationStarted",
  "OperationCompleted",
  "OperationFailed",
  "ParamSet",
  "ParamUnset",
  "ResourceStateChanged",
  "FaultFiled",
  "FaultCleared",
  "ScaleChanged",
  "DeploymentRestarted",
  "ResourceStopped",
  "ResourceUnstopped",
]);

export default function AppDetail() {
  const { name } = useParams<{ name: string }>();
  const navigate = useNavigate();
  const [removalOpen, setRemovalOpen] = useState(false);
  const { execute: executeUnstopAll, loading: unstoppingAll } = useOiAction();
  // i[impl action.cancel]
  const { execute: executeCancelAction, loading: cancellingAction } =
    useOiAction();
  const { data, loading, error, refetch } = useOiQuery<AppDetail>(
    "/apps/show",
    { app: name },
  );
  const { data: mappings, refetch: refetchMappings } = useOiQuery<
    ExternalMapping[]
  >("/volumes/external/list", { app: name });
  const [mapDialogState, setMapDialogState] = useState<
    | { mode: "prefill"; volName: string }
    | { mode: "remap"; existing: ExternalMapping }
    | null
  >(null);
  const matchesApp = useCallback(
    (ev: SeedlingEvent) =>
      APP_DETAIL_EVENTS.has(ev.type) && (!ev.app || ev.app === name),
    [name],
  );
  useEventRefresh(refetch, matchesApp);

  return (
    <Box sx={{ p: 3, maxWidth: 900, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography
          component={Link}
          to="/"
          variant="body2"
          sx={{
            color: "text.secondary",
            textDecoration: "none",
            "&:hover": { textDecoration: "underline" },
          }}
        >
          Apps
        </Typography>
        <Typography
          variant="body2"
          sx={{
            color: "text.disabled",
          }}
        >
          /
        </Typography>
        <Typography variant="body2">{name}</Typography>
        <Box sx={{ flexGrow: 1 }} />
        {data?.status === "not_installed" && (
          <OutlinedActionButton
            safety="dangerous"
            size="small"
            onClick={() => setRemovalOpen(true)}
            disabled={loading}
          >
            Deregister
          </OutlinedActionButton>
        )}
        {data?.status !== "not_installed" &&
          data?.status !== "installing" &&
          data?.status !== "uninstalling" &&
          data?.status !== "deregistering" && (
            <OutlinedActionButton
              safety="dangerous"
              size="small"
              onClick={() => setRemovalOpen(true)}
              disabled={loading}
            >
              Uninstall
            </OutlinedActionButton>
          )}
        <OutlinedActionButton
          safety="read"
          size="small"
          startIcon={<ArticleIcon />}
          onClick={() => navigate(`/apps/${name}/logs`)}
        >
          Logs
        </OutlinedActionButton>
        <OutlinedActionButton
          safety="write"
          size="small"
          startIcon={<EditIcon />}
          onClick={() => navigate(`/apps/${name}/script`)}
        >
          Edit script
        </OutlinedActionButton>
        <IconActionButton
          safety="read"
          tooltip="Refresh"
          onClick={refetch}
          disabled={loading}
        >
          <RefreshIcon />
        </IconActionButton>
      </Box>
      {error && <OiErrorAlert error={error} />}
      {loading && !data && (
        <Box sx={{ display: "flex", justifyContent: "center", mt: 4 }}>
          <CircularProgress />
        </Box>
      )}
      {data && (
        <Stack spacing={3}>
          <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
            <Typography variant="h5">{name}</Typography>
            <Chip
              label={statusLabel(
                data.status,
                data.current_operation?.action_name,
              )}
              color={statusColor(data.status)}
              size="small"
            />
            <Typography
              variant="caption"
              sx={{
                color: "text.secondary",
              }}
            >
              gen {data.generation}
            </Typography>
          </Box>

          {data.current_operation && (
            <Alert
              severity="info"
              action={
                <OutlinedActionButton
                  safety="dangerous"
                  size="small"
                  disabled={cancellingAction}
                  onClick={async () => {
                    try {
                      await executeCancelAction("/apps/action/cancel", {
                        app: name,
                      });
                      refetch();
                    } catch {
                      // surfaced by useOiAction globally
                    }
                  }}
                >
                  Cancel
                </OutlinedActionButton>
              }
            >
              Operation in progress:{" "}
              <strong>{data.current_operation.action_name}</strong> (gen{" "}
              {data.current_operation.source_generation} →{" "}
              {data.current_operation.target_generation})
              {data.current_operation.barrier && (
                <>
                  {" "}
                  · barrier: {data.current_operation.barrier.required_state} (
                  {Math.round(data.current_operation.barrier.elapsed_secs)}s
                  {data.current_operation.barrier.deadline_secs !== null
                    ? ` / ${data.current_operation.barrier.deadline_secs}s`
                    : " / ∞"}
                  )
                </>
              )}
            </Alert>
          )}

          {data.stopped_resources.length > 0 && (
            <Alert
              severity="warning"
              action={
                <OutlinedActionButton
                  safety="write"
                  size="small"
                  disabled={unstoppingAll}
                  onClick={async () => {
                    try {
                      await executeUnstopAll("/apps/unstop", { app: name });
                      refetch();
                    } catch {
                      // surfaced by useOiAction globally
                    }
                  }}
                >
                  Unstop all
                </OutlinedActionButton>
              }
            >
              Partially running —{" "}
              {data.stopped_resources
                .map((r) => `${r.kind}/${r.name}`)
                .join(", ")}{" "}
              {data.stopped_resources.length === 1 ? "is" : "are"} stopped.
            </Alert>
          )}

          {data.faults.length > 0 && (
            <Section
              title="Faults"
              action={
                <ClearFaultsButton
                  appName={name!}
                  status={data.status}
                  onCleared={refetch}
                />
              }
            >
              <FaultList faults={data.faults} />
            </Section>
          )}

          <Divider />

          <Section title="Params">
            <ParamsSection
              appName={name!}
              params={data.params}
              status={data.status}
              onRefresh={refetch}
            />
          </Section>

          {data.resources.some((r) => r.def?.kind === "external_volume") && (
            <>
              <Divider />
              <Section title="External Volumes">
                <TableContainer component={Paper} variant="outlined">
                  <Table size="small">
                    <TableHead>
                      <TableRow>
                        <TableCell>Name</TableCell>
                        <TableCell>Mapped to</TableCell>
                        <TableCell width={80} align="right" />
                      </TableRow>
                    </TableHead>
                    <TableBody>
                      {data.resources
                        .filter((r) => r.def?.kind === "external_volume")
                        .map((r) => {
                          const mapping = (mappings ?? []).find(
                            (m) => m.external_name === r.name,
                          );
                          return (
                            <TableRow key={r.name}>
                              <TableCell sx={{ fontFamily: "monospace" }}>
                                {r.name}
                              </TableCell>
                              <TableCell>
                                {mapping ? (
                                  <Box
                                    sx={{
                                      display: "flex",
                                      alignItems: "center",
                                      gap: 0.5,
                                    }}
                                  >
                                    <Typography
                                      variant="caption"
                                      sx={{ fontFamily: "monospace" }}
                                    >
                                      {mapping.target.kind === "app"
                                        ? `${mapping.target.app}/${mapping.target.volume}`
                                        : mapping.target.name}
                                    </Typography>
                                    <Chip
                                      label={mapping.target.kind}
                                      size="small"
                                      variant="outlined"
                                    />
                                    {mapping.read_only && (
                                      <Chip
                                        label="ro"
                                        size="small"
                                        variant="outlined"
                                      />
                                    )}
                                  </Box>
                                ) : (
                                  <Typography
                                    variant="caption"
                                    sx={{
                                      color: "text.secondary",
                                    }}
                                  >
                                    Not mapped
                                  </Typography>
                                )}
                              </TableCell>
                              <TableCell align="right">
                                <OutlinedActionButton
                                  safety="write"
                                  size="small"
                                  onClick={() =>
                                    setMapDialogState(
                                      mapping
                                        ? {
                                            mode: "remap",
                                            existing: mapping,
                                          }
                                        : {
                                            mode: "prefill",
                                            volName: r.name,
                                          },
                                    )
                                  }
                                >
                                  {mapping ? "Remap" : "Map"}
                                </OutlinedActionButton>
                              </TableCell>
                            </TableRow>
                          );
                        })}
                    </TableBody>
                  </Table>
                </TableContainer>
              </Section>
            </>
          )}

          <Divider />

          {data.status === "not_installed" ? (
            <InstallSection
              appName={name!}
              installAction={data.actions.find((a) => a.kind === "install")}
              hasScriptError={data.faults.some(
                (f) => f.kind === "script_error",
              )}
              faults={data.faults}
              onRefresh={refetch}
            />
          ) : (
            <>
              {data.status === "uninstalling" && (
                <Alert severity="info" icon={<CircularProgress size={16} />}>
                  Uninstalling — tearing down resources. The app will reappear
                  as not-installed once teardown completes.
                </Alert>
              )}
              <Box
                sx={{
                  opacity: data.status === "uninstalling" ? 0.5 : 1,
                  pointerEvents:
                    data.status === "uninstalling" ? "none" : "auto",
                  transition: "opacity 0.15s",
                }}
              >
                <Section title="Actions">
                  <ActionsSection
                    appName={name!}
                    actions={data.actions}
                    status={data.status}
                    hasScriptError={data.faults.some(
                      (f) => f.kind === "script_error",
                    )}
                    operatingAction={data.current_operation?.action_name}
                    onRefresh={refetch}
                  />
                </Section>

                {data.actions.some((a) => a.schedules.length > 0) && (
                  <>
                    <Divider sx={{ my: 3 }} />
                    <Section title="Schedules">
                      <SchedulesSection actions={data.actions} />
                    </Section>
                  </>
                )}

                <Divider sx={{ my: 3 }} />

                <Section title="Resources">
                  <ResourcesSection
                    appName={name!}
                    resources={[
                      ...data.resources,
                      ...(data.dynamic_resources ?? []).map((r) => ({
                        ...r,
                        dynamic: true,
                      })),
                    ]}
                    onRefresh={refetch}
                  />
                </Section>

                {data.resources.some(
                  (r) => r.def?.kind === "ingress" && r.def.tls,
                ) && (
                  <>
                    <Divider sx={{ my: 3 }} />
                    <Section title="TLS certificates">
                      <TlsHostnamesTable app={name!} hideAppsColumn hideTitle />
                    </Section>
                  </>
                )}

                <Divider sx={{ my: 3 }} />

                <AppImagesSection
                  appName={name!}
                  resources={data.resources}
                  onRefresh={refetch}
                />
              </Box>
            </>
          )}
        </Stack>
      )}
      {mapDialogState && (
        <MapVolumeDialog
          open={true}
          onClose={() => setMapDialogState(null)}
          onSuccess={() => {
            setMapDialogState(null);
            void refetchMappings();
          }}
          {...(mapDialogState.mode === "remap"
            ? { existing: mapDialogState.existing }
            : { prefill: { app: name!, name: mapDialogState.volName } })}
        />
      )}
      <AppRemovalDialog
        appName={name!}
        kind={data?.status === "not_installed" ? "deregister" : "uninstall"}
        open={removalOpen}
        onClose={() => setRemovalOpen(false)}
        onSuccess={() => {
          setRemovalOpen(false);
          if (data?.status === "not_installed") navigate("/");
          else refetch();
        }}
      />
    </Box>
  );
}
