import AddIcon from "@mui/icons-material/Add";
import ArticleIcon from "@mui/icons-material/Article";
import CameraAltIcon from "@mui/icons-material/CameraAlt";
import CheckIcon from "@mui/icons-material/Check";
import ClearIcon from "@mui/icons-material/Clear";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutline";
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
  DialogTitle,
  Divider,
  IconButton,
  InputAdornment,
  Paper,
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
import { useCallback, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { MapVolumeDialog } from "../components/MapVolumeDialog";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useSessionContext } from "../components/SessionProvider";
import { SnapshotVolumeDialog } from "../components/SnapshotVolumeDialog";
import { useOiAction } from "../hooks/useOiAction";
import { useOiQuery } from "../hooks/useOi";
import { useEventRefresh } from "../hooks/useEventRefresh";
import { isStrongPassword, passwordScore } from "../lib/passwordStrength";
import { statusColor, statusLabel } from "../lib/status";
import type {
  AppAction,
  AppDetail,
  AppParam,
  AppResource,
  AppStatus,
  ExternalMapping,
  FaultRecord,
  InstallRequirement,
  ResourceDef,
  SeedlingEvent,
} from "../lib/types";

function lifecycleColor(
  state: string,
): "success" | "warning" | "error" | "default" {
  if (state === "ready" || state === "active") return "success";
  if (state === "failed") return "error";
  if (state === "excluded") return "warning";
  return "default";
}

function FaultList({ faults, showApp }: { faults: FaultRecord[]; showApp?: boolean }) {
  if (faults.length === 0) return null;
  return (
    <Stack spacing={1}>
      {faults.map((f) => (
        <Alert key={f.id} severity="error" sx={{ fontFamily: "monospace" }}>
          <Box sx={{ display: "flex", justifyContent: "space-between", gap: 2, flexWrap: "wrap" }}>
            <Box>
              {showApp && f.app && <><Link to={`/apps/${f.app}`} style={{ color: "inherit" }}>{f.app}</Link>{" · "}</>}
              <strong>{f.kind}</strong>
              {f.resource_name && ` · ${f.resource_type}/${f.resource_name}`}
              {f.instance_id && ` (${f.instance_id.slice(0, 12)})`}
              {" — "}
              {f.description}
            </Box>
            <Typography variant="caption" color="text.secondary" sx={{ whiteSpace: "nowrap", alignSelf: "center" }}>
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
      <Box sx={{ mt: 0.5, display: "flex", gap: 0.5, flexWrap: "wrap", alignItems: "center" }}>
        <Typography variant="caption" sx={{ fontFamily: "monospace", mr: 0.5 }}>{url}</Typography>
        {def.http_terminate && <Chip label={def.http_terminate} size="small" variant="outlined" />}
        {def.dtls && <Chip label="dtls" size="small" variant="outlined" />}
        {def.redirect && (
          <Chip label={`redirect :${def.redirect.port} (${def.redirect.code})`} size="small" variant="outlined" />
        )}
      </Box>
    );
  }
  if (def.kind === "service") {
    if (!def.http) return null;
    return <Chip label="http" size="small" variant="outlined" sx={{ mt: 0.5 }} />;
  }
  if (def.kind === "http_service") {
    return (
      <Typography variant="caption" sx={{ fontFamily: "monospace", display: "block", mt: 0.5 }}>
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
      <Box sx={{ mt: 0.5, display: "flex", gap: 0.5, flexWrap: "wrap", alignItems: "center" }}>
        {def.container.image && (
          <Typography
            variant="caption"
            sx={{ fontFamily: "monospace", opacity: 0.8, maxWidth: 400, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
            title={def.container.image}
          >
            {def.container.image}
          </Typography>
        )}
        {bindings.map((b) => <Chip key={b} label={b} size="small" variant="outlined" />)}
        {def.container.memory && <Chip label={`mem: ${def.container.memory}`} size="small" variant="outlined" />}
        {def.container.cpus != null && <Chip label={`cpu: ${def.container.cpus}`} size="small" variant="outlined" />}
        {def.kind === "job" && def.deadline != null && (
          <Chip label={`deadline: ${def.deadline}s`} size="small" variant="outlined" />
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
        {chips.map((c) => <Chip key={c} label={c} size="small" variant="outlined" />)}
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
  const { execute, loading: scaling } = useOiAction();
  const { execute: executeRestart, loading: restarting } = useOiAction();
  const { execute: executeStop, loading: stopping } = useOiAction();
  const { openVolumeShell } = useSessionContext();
  const [snapshotTarget, setSnapshotTarget] = useState<
    { source: string; label: string } | null
  >(null);

  const scale = async (deploymentName: string, value: number) => {
    try {
      await execute("/apps/scale", { app: appName, deployment: deploymentName, scale: value });
      onRefresh();
    } catch {
      // errors surfaced by useOiAction globally
    }
  };

  const restart = async (deploymentName: string) => {
    try {
      await executeRestart("/apps/restart", { app: appName, deployment: deploymentName });
    } catch {
      // errors surfaced by useOiAction globally
    }
  };

  const stopResource = async (kind: string, resourceName: string) => {
    try {
      await executeStop("/apps/resource/stop", { app: appName, kind, name: resourceName });
      onRefresh();
    } catch {
      // errors surfaced by useOiAction globally
    }
  };

  const unstopResource = async (kind: string, resourceName: string) => {
    try {
      await executeStop("/apps/resource/unstop", { app: appName, kind, name: resourceName });
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
        <Typography color="text.secondary">No resources.</Typography>
        {snapshotDialog}
      </>
    );
  return (
    <Stack spacing={2}>
      {resources.map((r) => (
        <Box key={`${r.type}/${r.name}`}>
          <Box sx={{ display: "flex", alignItems: "center", gap: 1, mb: 0.5 }}>
            <Typography variant="subtitle2">{r.name}</Typography>
            <Typography variant="caption" color="text.secondary">
              {r.type}
            </Typography>
            {r.stopped && (
              <Chip label="stopped" size="small" color="warning" variant="outlined"
                sx={{ fontSize: "0.65rem", height: 18, "& .MuiChip-label": { px: 0.75 } }}
              />
            )}
            {(r.type === "deployment" || r.type === "job") && (
              <Tooltip title="View resource logs">
                <IconButton
                  size="small"
                  component={Link}
                  to={`/apps/${appName}/logs?resource=${r.name}`}
                >
                  <ArticleIcon sx={{ fontSize: 14 }} />
                </IconButton>
              </Tooltip>
            )}
            {r.scale && (
              <>
                <Typography variant="caption" color="text.secondary">
                  · scale
                </Typography>
                <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
                  <Tooltip title="Scale down">
                    <span>
                      <IconButton
                        size="small"
                        onClick={() => void scale(r.name, r.scale!.current - 1)}
                        disabled={scaling || r.scale.current <= r.scale.low}
                      >
                        <RemoveIcon sx={{ fontSize: 14 }} />
                      </IconButton>
                    </span>
                  </Tooltip>
                  <Typography variant="caption">
                    {r.scale.current}
                  </Typography>
                  <Tooltip title="Scale up">
                    <span>
                      <IconButton
                        size="small"
                        onClick={() => void scale(r.name, r.scale!.current + 1)}
                        disabled={scaling || r.scale.current >= r.scale.high}
                      >
                        <AddIcon sx={{ fontSize: 14 }} />
                      </IconButton>
                    </span>
                  </Tooltip>
                  <Typography variant="caption" color="text.secondary">
                    [{r.scale.low}–{r.scale.high}]
                  </Typography>
                </Box>
              </>
            )}
            {r.type === "deployment" && (
              <Tooltip title="Restart deployment">
                <span>
                  <IconButton
                    size="small"
                    onClick={() => void restart(r.name)}
                    disabled={restarting}
                  >
                    <RefreshIcon sx={{ fontSize: 14 }} />
                  </IconButton>
                </span>
              </Tooltip>
            )}
            {/* w[volumes.shell-ui] */}
            {r.type === "volume" && (
              <>
                <Tooltip title="Open shell">
                  <IconButton
                    size="small"
                    onClick={() => openVolumeShell(
                      [{ kind: "app", app: appName, volume: r.name }],
                      `${appName}.${r.name}`,
                    )}
                  >
                    <TerminalIcon sx={{ fontSize: 14 }} />
                  </IconButton>
                </Tooltip>
                <Tooltip title="Snapshot">
                  <IconButton
                    size="small"
                    onClick={() =>
                      setSnapshotTarget({
                        source: `${appName}/${r.name}`,
                        label: `${appName}/${r.name}`,
                      })
                    }
                  >
                    <CameraAltIcon sx={{ fontSize: 14 }} />
                  </IconButton>
                </Tooltip>
              </>
            )}
            {STOPPABLE_KINDS.has(r.type) && (
              r.stopped ? (
                <Tooltip title="Unstop resource">
                  <span>
                    <IconButton
                      size="small"
                      onClick={() => void unstopResource(r.type, r.name)}
                      disabled={stopping}
                      color="success"
                    >
                      <PlayArrowIcon sx={{ fontSize: 14 }} />
                    </IconButton>
                  </span>
                </Tooltip>
              ) : (
                <Tooltip title="Stop resource">
                  <span>
                    <IconButton
                      size="small"
                      onClick={() => void stopResource(r.type, r.name)}
                      disabled={stopping}
                    >
                      <PauseIcon sx={{ fontSize: 14 }} />
                    </IconButton>
                  </span>
                </Tooltip>
              )
            )}
          </Box>
          <FaultList faults={r.faults} />
          {r.def && <ResourceDefDetail def={r.def} />}
          <TableContainer component={Paper} variant="outlined" sx={{ mt: 0.5 }}>
            <Table size="small">
              <TableHead>
                <TableRow>
                  <TableCell>Instance</TableCell>
                  <TableCell width={120} align="right">State</TableCell>
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
                      <TableCell width={120} align="right">
                        <Chip
                          label={inst.lifecycle.replace(/_/g, " ")}
                          color={lifecycleColor(inst.lifecycle)}
                          size="small"
                        />
                      </TableCell>
                      <TableCell width={40} align="right" sx={{ px: 0.5 }}>
                        {(r.type === "deployment" || r.type === "job") && (
                          <Tooltip title="View instance logs">
                            <IconButton
                              size="small"
                              component={Link}
                              to={`/apps/${appName}/logs?resource=${r.name}&instance=${inst.display_name}`}
                            >
                              <ArticleIcon sx={{ fontSize: 14 }} />
                            </IconButton>
                          </Tooltip>
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
  const editsDisabled = status === "installing" || status === "operating";
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
      await execute("/apps/params/set", { app: appName, name: addName.trim(), value: addValue });
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
          inputProps={{ style: { fontFamily: "monospace" } }}
          sx={{ width: 200 }}
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
          inputProps={{ style: { fontFamily: "monospace" } }}
        />
      </TableCell>
      <TableCell align="right" sx={{ whiteSpace: "nowrap" }}>
        <Tooltip title="Save">
          <span>
            <IconButton size="small" onClick={() => void saveAdd()} disabled={loading || !addName.trim()}>
              <CheckIcon fontSize="small" />
            </IconButton>
          </span>
        </Tooltip>
        <Tooltip title="Cancel">
          <IconButton size="small" onClick={cancelAdd}>
            <ClearIcon fontSize="small" />
          </IconButton>
        </Tooltip>
      </TableCell>
    </TableRow>
  ) : null;

  const disabledBanner = editsDisabled ? (
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
          <Typography color="text.secondary">No params.</Typography>
          <Button
            size="small"
            startIcon={<AddIcon fontSize="small" />}
            onClick={startAdd}
            disabled={editsDisabled}
          >
            Set param
          </Button>
        </Box>
        {adding && (
          <TableContainer component={Paper} variant="outlined">
            <Table size="small"><TableBody>{addRow}</TableBody></Table>
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
          <Button
            size="small"
            startIcon={<AddIcon fontSize="small" />}
            onClick={startAdd}
            disabled={editsDisabled}
          >
            Set param
          </Button>
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
                  <TableCell sx={{ fontFamily: "monospace" }}>{p.name}</TableCell>
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
                        if (e.key === "Enter") void save();
                        if (e.key === "Escape") cancel();
                      }}
                      autoFocus
                      type={showPassword ? "text" : paramFieldType(p.kind, p.secret)}
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
                      inputProps={{ style: { fontFamily: "monospace" } }}
                      InputProps={{
                        endAdornment: (
                          <InputAdornment position="end">
                            {(p.secret || p.kind === "password" ||
                              p.kind === "weak-password") && (
                              <Tooltip
                                title={
                                  showPassword ? "Hide" : "Show"
                                }
                              >
                                <IconButton
                                  size="small"
                                  onClick={() =>
                                    setShowPassword((v) => !v)
                                  }
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
                        <Typography component="span" color="error" sx={{ ml: 0.5 }}>*</Typography>
                      )}
                    </Box>
                    {p.description && (
                      <Typography variant="caption" color="text.secondary">
                        {p.description}
                      </Typography>
                    )}
                  </TableCell>
                  <TableCell>
                    <Chip label={p.kind} size="small" variant="outlined" />
                  </TableCell>
                  <TableCell sx={{ fontFamily: "monospace" }}>
                    {(() => {
                      const isMasked = p.secret || p.kind === "password" || p.kind === "weak-password";
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
                            <Typography component="span" variant="caption" sx={{ ml: 0.5 }}>
                              (default)
                            </Typography>
                          </Box>
                        );
                      }
                      return <Box component="span" sx={{ color: "text.disabled" }}>—</Box>;
                    })()}
                  </TableCell>
                  <TableCell align="right" sx={{ whiteSpace: "nowrap" }}>
                    <Tooltip title={p.value == null && !p.is_set ? "Set" : "Edit"}>
                      <span>
                        <IconButton
                          size="small"
                          onClick={() => startEdit(p)}
                          disabled={loading || editsDisabled}
                        >
                          <EditIcon fontSize="small" />
                        </IconButton>
                      </span>
                    </Tooltip>
                    {p.value != null && !p.required && (
                      <Tooltip title="Unset">
                        <span>
                          <IconButton
                            size="small"
                            onClick={() => void unset(p.name)}
                            disabled={loading || editsDisabled}
                          >
                            <DeleteOutlineIcon fontSize="small" />
                          </IconButton>
                        </span>
                      </Tooltip>
                    )}
                    {p.value != null && p.required && p.default_value != null && (
                      <Tooltip title="Reset to default">
                        <span>
                          <IconButton
                            size="small"
                            onClick={() => void unset(p.name)}
                            disabled={loading || editsDisabled}
                          >
                            <RestoreIcon fontSize="small" />
                          </IconButton>
                        </span>
                      </Tooltip>
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
  if (secret || kind === "password" || kind === "weak-password") return "password";
  if (kind === "email") return "email";
  return "text";
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
      Object.entries(action.params).map(([k, def]: [string, InstallRequirement]) => [
        k,
        def.default_value ?? "",
      ]),
    ),
  );
  const [showPasswords, setShowPasswords] = useState<Record<string, boolean>>({});

  const toggleShow = (key: string) =>
    setShowPasswords((s) => ({ ...s, [key]: !s[key] }));

  const handleClose = () => {
    clearError();
    onClose();
  };

  const handleSubmit = async () => {
    const method =
      action.kind === "install" ? "/apps/install/invoke" : "/apps/action/invoke";
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

  const paramEntries = Object.entries(action.params) as [string, InstallRequirement][];

  const hasWeakPassword = paramEntries.some(
    ([key, def]) =>
      def.kind === "password" &&
      values[key] != null &&
      !isStrongPassword(values[key]),
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
            <Typography variant="body2" color="text.secondary">
              No params required.
            </Typography>
          ) : (
            paramEntries.map(([key, def]) => {
              const isPassword =
                def.kind === "password" || def.kind === "weak-password";
              const val = values[key] ?? "";
              const weak =
                def.kind === "password" && val.length > 0 && !isStrongPassword(val);
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
                  type={showPasswords[key] ? "text" : paramFieldType(def.kind)}
                  required={def.required}
                  inputProps={{ style: { fontFamily: "monospace" } }}
                  InputProps={
                    isPassword
                      ? {
                          endAdornment: (
                            <InputAdornment position="end">
                              <Tooltip title={showPasswords[key] ? "Hide" : "Show"}>
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
                            </InputAdornment>
                          ),
                        }
                      : undefined
                  }
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
        <Button
          variant="contained"
          onClick={handleSubmit}
          disabled={loading || hasWeakPassword}
        >
          {loading
            ? "Running…"
            : action.kind === "install"
              ? "Install"
              : "Run"}
        </Button>
      </DialogActions>
    </Dialog>
  );
}

function InstallingSection({
  appName,
  faults,
}: {
  appName: string;
  faults: FaultRecord[];
}) {
  const installFaults = faults.filter((f) => f.kind === "operation_failed");
  return (
    <Box
      sx={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        gap: 2,
        py: 6,
      }}
    >
      <CircularProgress />
      <Typography color="text.secondary" sx={{ textAlign: "center" }}>
        Install in progress — the runtime is actuating your resources.
      </Typography>
      <Button
        size="small"
        startIcon={<ArticleIcon />}
        component={Link}
        to={`/apps/${appName}/logs`}
      >
        View container logs
      </Button>
      {installFaults.length > 0 && (
        <Alert severity="error" sx={{ width: "100%", mt: 1 }}>
          <Typography variant="subtitle2" gutterBottom>
            The install is currently failing:
          </Typography>
          {installFaults.map((f) => (
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
    </Box>
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

  const hasParams = installAction && Object.keys(installAction.params).length > 0;
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
        <Typography color="text.secondary">
          This app has not been installed yet.
        </Typography>
        <Button
          variant="contained"
          size="large"
          onClick={() => void handleInstall()}
          disabled={loading || hasScriptError}
        >
          {loading ? "Installing…" : "Install"}
        </Button>
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

  const canInstall = status === "not_installed" && !hasScriptError;
  const canInvoke =
    !hasScriptError &&
    status !== "not_installed" &&
    status !== "installing" &&
    status !== "uninstalling" &&
    status !== "deregistering" &&
    status !== "operating";

  if (actions.length === 0)
    return <Typography color="text.secondary">No actions.</Typography>;

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
            {actions.map((a) => {
              const isInvokable = a.kind !== "shell" && a.kind !== "lifecycle";
              const isRunning = a.name === operatingAction;
              const canRun =
                a.kind === "install"
                  ? canInstall
                  : isInvokable && canInvoke;
              return (
                <TableRow key={a.name}>
                  <TableCell sx={{ fontFamily: "monospace" }}>{a.name}</TableCell>
                  <TableCell>
                    <Chip label={a.kind} size="small" variant="outlined" />
                  </TableCell>
                  <TableCell sx={{ color: "text.secondary" }}>
                    {a.description}
                  </TableCell>
                  <TableCell align="right">
                    {/* w[shells.ui] */}
                    {a.kind === "shell" ? (
                      <Button
                        size="small"
                        variant="outlined"
                        onClick={() => {
                          if (Object.keys(a.params).length > 0) {
                            setOpeningShell(a);
                          } else {
                            openShell(appName, a.name, {});
                          }
                        }}
                        disabled={!canInvoke}
                      >
                        Open shell
                      </Button>
                    ) : isInvokable && (
                      isRunning ? (
                        <Button
                          size="small"
                          variant="outlined"
                          disabled
                          startIcon={<CircularProgress size={12} />}
                        >
                          Running…
                        </Button>
                      ) : (
                        <Button
                          size="small"
                          variant={a.kind === "install" ? "contained" : "outlined"}
                          onClick={() => setInvoking(a)}
                          disabled={!canRun}
                        >
                          {a.kind === "install" ? "Install" : "Run"}
                        </Button>
                      )
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
      Object.entries(action.params).map(([k, def]: [string, InstallRequirement]) => [
        k,
        def.default_value ?? "",
      ]),
    ),
  );
  const [showPasswords, setShowPasswords] = useState<Record<string, boolean>>({});

  const toggleShow = (key: string) =>
    setShowPasswords((s) => ({ ...s, [key]: !s[key] }));

  const paramEntries = Object.entries(action.params) as [string, InstallRequirement][];
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
            const isPassword = def.kind === "password" || def.kind === "weak-password";
            const val = values[key] ?? "";
            const weak = def.kind === "password" && val.length > 0 && !isStrongPassword(val);
            const helperText =
              def.kind === "password" && val.length > 0
                ? weak ? "Password is too weak" : (def.description ?? undefined)
                : def.kind === "weak-password" && val.length > 0
                  ? `Strength: ${passwordScore(val)}/4${def.description ? ` — ${def.description}` : ""}`
                  : (def.description ?? undefined);
            return (
              <TextField
                key={key}
                label={key}
                size="small"
                value={val}
                onChange={(e) => setValues((v) => ({ ...v, [key]: e.target.value }))}
                helperText={helperText}
                error={weak}
                type={showPasswords[key] ? "text" : paramFieldType(def.kind)}
                required={def.required}
                inputProps={{ style: { fontFamily: "monospace" } }}
                InputProps={
                  isPassword
                    ? {
                        endAdornment: (
                          <InputAdornment position="end">
                            <Tooltip title={showPasswords[key] ? "Hide" : "Show"}>
                              <IconButton size="small" onClick={() => toggleShow(key)} edge="end">
                                {showPasswords[key] ? (
                                  <VisibilityOffIcon fontSize="small" />
                                ) : (
                                  <VisibilityIcon fontSize="small" />
                                )}
                              </IconButton>
                            </Tooltip>
                          </InputAdornment>
                        ),
                      }
                    : undefined
                }
              />
            );
          })}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose}>Cancel</Button>
        <Button
          variant="contained"
          onClick={() => onOpen(values)}
          disabled={hasWeakPassword}
        >
          Open shell
        </Button>
      </DialogActions>
    </Dialog>
  );
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <Box>
      <Typography variant="h6" sx={{ mb: 1 }}>
        {title}
      </Typography>
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

  const handleClose = () => { clearError(); onClose(); };

  return (
    <Dialog open={open} onClose={handleClose} maxWidth="xs" fullWidth>
      <DialogTitle>{kind === "uninstall" ? "Uninstall app" : "Deregister app"}</DialogTitle>
      <DialogContent>
        {error && <OiErrorAlert error={error} />}
        <Typography>
          {kind === "uninstall" ? (
            <>Uninstall <strong>{appName}</strong>? This will tear down all its resources. The app will remain registered and can be reinstalled.</>
          ) : (
            <>Remove <strong>{appName}</strong> from Seedling entirely? This cannot be undone.</>
          )}
        </Typography>
      </DialogContent>
      <DialogActions>
        <Button onClick={handleClose} disabled={loading}>Cancel</Button>
        <Button
          variant="contained"
          color="error"
          onClick={handleConfirm}
          disabled={loading}
        >
          {loading
            ? kind === "uninstall" ? "Uninstalling…" : "Removing…"
            : kind === "uninstall" ? "Uninstall" : "Deregister"}
        </Button>
      </DialogActions>
    </Dialog>
  );
}

const APP_DETAIL_EVENTS: Set<string> = new Set([
  "AppUpdated", "AppPhaseChanged",
  "OperationStarted", "OperationCompleted", "OperationFailed",
  "ParamSet", "ParamUnset", "ResourceStateChanged", "FaultFiled", "FaultCleared",
  "ScaleChanged", "DeploymentRestarted", "ResourceStopped", "ResourceUnstopped",
]);

export default function AppDetail() {
  const { name } = useParams<{ name: string }>();
  const navigate = useNavigate();
  const [removalOpen, setRemovalOpen] = useState(false);
  const { execute: executeUnstopAll, loading: unstoppingAll } = useOiAction();
  const { data, loading, error, refetch } = useOiQuery<AppDetail>(
    "/apps/show",
    { app: name },
  );
  const { data: mappings, refetch: refetchMappings } = useOiQuery<ExternalMapping[]>(
    "/volumes/external/list",
    { app: name },
  );
  const [mapDialogState, setMapDialogState] = useState<
    | { mode: "prefill"; volName: string }
    | { mode: "remap"; existing: ExternalMapping }
    | null
  >(null);
  const matchesApp = useCallback(
    (ev: SeedlingEvent) => APP_DETAIL_EVENTS.has(ev.type) && (!ev.app || ev.app === name),
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
        <Typography variant="body2" color="text.disabled">
          /
        </Typography>
        <Typography variant="body2">{name}</Typography>
        <Box sx={{ flexGrow: 1 }} />
        {data?.status === "not_installed" && (
          <Button
            size="small"
            color="error"
            onClick={() => setRemovalOpen(true)}
            disabled={loading}
          >
            Deregister
          </Button>
        )}
        {data?.status !== "not_installed" &&
          data?.status !== "installing" &&
          data?.status !== "uninstalling" &&
          data?.status !== "deregistering" && (
          <Button
            size="small"
            color="error"
            onClick={() => setRemovalOpen(true)}
            disabled={loading}
          >
            Uninstall
          </Button>
        )}
        <Button
          size="small"
          startIcon={<ArticleIcon />}
          component={Link}
          to={`/apps/${name}/logs`}
        >
          Logs
        </Button>
        <Button
          size="small"
          startIcon={<EditIcon />}
          onClick={() => navigate(`/apps/${name}/script`)}
        >
          Edit script
        </Button>
        <Tooltip title="Refresh">
          <span>
            <IconButton onClick={refetch} disabled={loading} size="small">
              <RefreshIcon />
            </IconButton>
          </span>
        </Tooltip>
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
            <Typography variant="caption" color="text.secondary">
              gen {data.generation}
            </Typography>
          </Box>

          {data.current_operation && (
            <Alert severity="info">
              Operation in progress:{" "}
              <strong>{data.current_operation.action_name}</strong>{" "}
              (gen {data.current_operation.source_generation} →{" "}
              {data.current_operation.target_generation})
              {data.current_operation.barrier && (
                <>
                  {" "}· barrier: {data.current_operation.barrier.required_state}{" "}
                  ({Math.round(data.current_operation.barrier.elapsed_secs)}s /{" "}
                  {data.current_operation.barrier.deadline_secs}s)
                </>
              )}
            </Alert>
          )}

          {data.stopped_resources.length > 0 && (
            <Alert
              severity="warning"
              action={
                <Button
                  color="inherit"
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
                </Button>
              }
            >
              Partially running —{" "}
              {data.stopped_resources.map((r) => `${r.kind}/${r.name}`).join(", ")}{" "}
              {data.stopped_resources.length === 1 ? "is" : "are"} stopped.
            </Alert>
          )}

          {data.faults.length > 0 && (
            <Section title="Faults">
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
                                  <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
                                    <Typography variant="caption" sx={{ fontFamily: "monospace" }}>
                                      {mapping.target_kind === "exported" && mapping.target_app
                                        ? `${mapping.target_app}/${mapping.target_volume}`
                                        : mapping.target_volume}
                                    </Typography>
                                    <Chip label={mapping.target_kind} size="small" variant="outlined" />
                                    {mapping.read_only && (
                                      <Chip label="ro" size="small" variant="outlined" />
                                    )}
                                  </Box>
                                ) : (
                                  <Typography variant="caption" color="text.secondary">
                                    Not mapped
                                  </Typography>
                                )}
                              </TableCell>
                              <TableCell align="right">
                                <Button
                                  size="small"
                                  onClick={() =>
                                    setMapDialogState(
                                      mapping
                                        ? { mode: "remap", existing: mapping }
                                        : { mode: "prefill", volName: r.name },
                                    )
                                  }
                                >
                                  {mapping ? "Remap" : "Map"}
                                </Button>
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
              hasScriptError={data.faults.some((f) => f.kind === "script_error")}
              faults={data.faults}
              onRefresh={refetch}
            />
          ) : data.status === "installing" ? (
            <InstallingSection appName={name!} faults={data.faults} />
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
                  pointerEvents: data.status === "uninstalling" ? "none" : "auto",
                  transition: "opacity 0.15s",
                }}
              >
                <Section title="Actions">
                  <ActionsSection
                    appName={name!}
                    actions={data.actions}
                    status={data.status}
                    hasScriptError={data.faults.some((f) => f.kind === "script_error")}
                    operatingAction={data.current_operation?.action_name}
                    onRefresh={refetch}
                  />
                </Section>

                <Divider sx={{ my: 3 }} />

                <Section title="Resources">
                  <ResourcesSection
                    appName={name!}
                    resources={data.resources}
                    onRefresh={refetch}
                  />
                </Section>
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
            : { prefill: { app: name!, name: mapDialogState.volName } }
          )}
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
