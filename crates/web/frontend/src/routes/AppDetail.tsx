import AddIcon from "@mui/icons-material/Add";
import ArticleIcon from "@mui/icons-material/Article";
import CheckIcon from "@mui/icons-material/Check";
import ClearIcon from "@mui/icons-material/Clear";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutline";
import EditIcon from "@mui/icons-material/Edit";
import RefreshIcon from "@mui/icons-material/Refresh";
import RemoveIcon from "@mui/icons-material/Remove";
import RestoreIcon from "@mui/icons-material/Restore";
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
import { OiErrorAlert } from "../components/OiErrorAlert";
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

  const scale = async (deploymentName: string, value: number) => {
    try {
      await execute("/apps/scale", { app: appName, deployment: deploymentName, scale: value });
      onRefresh();
    } catch {
      // errors surfaced by useOiAction globally
    }
  };

  if (resources.length === 0)
    return <Typography color="text.secondary">No resources.</Typography>;
  return (
    <Stack spacing={2}>
      {resources.map((r) => (
        <Box key={`${r.type}/${r.name}`}>
          <Box sx={{ display: "flex", alignItems: "center", gap: 1, mb: 0.5 }}>
            <Typography variant="subtitle2">{r.name}</Typography>
            <Typography variant="caption" color="text.secondary">
              {r.type}
            </Typography>
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
    </Stack>
  );
}

function ParamsSection({
  appName,
  params,
  onRefresh,
}: {
  appName: string;
  params: AppParam[];
  onRefresh: () => void;
}) {
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

  if (params.length === 0 && !adding) {
    return (
      <Stack spacing={1}>
        {error && <OiErrorAlert error={error} />}
        <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
          <Typography color="text.secondary">No params.</Typography>
          <Button size="small" startIcon={<AddIcon fontSize="small" />} onClick={startAdd}>
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
      {error && <OiErrorAlert error={error} />}
      <Box sx={{ display: "flex", justifyContent: "flex-end" }}>
        {!adding && (
          <Button size="small" startIcon={<AddIcon fontSize="small" />} onClick={startAdd}>
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
                      type={showPassword ? "text" : paramFieldType(p.kind)}
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
                            {(p.kind === "password" ||
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
                      const isPassword = p.kind === "password" || p.kind === "weak-password";
                      if (p.value != null) {
                        return isPassword ? "••••••••" : p.value;
                      }
                      if (p.default_value != null) {
                        return (
                          <Box component="span" sx={{ color: "text.disabled" }}>
                            {isPassword ? "••••••••" : p.default_value}
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
                    <Tooltip title={p.value == null ? "Set" : "Edit"}>
                      <IconButton
                        size="small"
                        onClick={() => startEdit(p)}
                        disabled={loading}
                      >
                        <EditIcon fontSize="small" />
                      </IconButton>
                    </Tooltip>
                    {p.value != null && !p.required && (
                      <Tooltip title="Unset">
                        <IconButton
                          size="small"
                          onClick={() => void unset(p.name)}
                          disabled={loading}
                        >
                          <DeleteOutlineIcon fontSize="small" />
                        </IconButton>
                      </Tooltip>
                    )}
                    {p.value != null && p.required && p.default_value != null && (
                      <Tooltip title="Reset to default">
                        <IconButton
                          size="small"
                          onClick={() => void unset(p.name)}
                          disabled={loading}
                        >
                          <RestoreIcon fontSize="small" />
                        </IconButton>
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

function paramFieldType(kind: string): string {
  if (kind === "password" || kind === "weak-password") return "password";
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

function InstallSection({
  appName,
  installAction,
  hasScriptError,
  onRefresh,
}: {
  appName: string;
  installAction: AppAction | undefined;
  hasScriptError: boolean;
  onRefresh: () => void;
}) {
  const { execute, loading } = useOiAction();
  const [dialogOpen, setDialogOpen] = useState(false);

  const hasParams = installAction && Object.keys(installAction.params).length > 0;

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

  const canInstall = status === "not_installed" && !hasScriptError;
  const canInvoke =
    !hasScriptError &&
    status !== "not_installed" &&
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
                    {isInvokable && (
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
    </>
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
  "AppUpdated", "OperationStarted", "OperationCompleted", "OperationFailed",
  "ParamSet", "ParamUnset", "ResourceStateChanged", "FaultFiled", "FaultCleared",
  "ScaleChanged",
]);

export default function AppDetail() {
  const { name } = useParams<{ name: string }>();
  const navigate = useNavigate();
  const [removalOpen, setRemovalOpen] = useState(false);
  const { data, loading, error, refetch } = useOiQuery<AppDetail>(
    "/apps/show",
    { app: name },
  );
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
              onRefresh={refetch}
            />
          </Section>

          <Divider />

          {data.status === "not_installed" ? (
            <InstallSection
              appName={name!}
              installAction={data.actions.find((a) => a.kind === "install")}
              hasScriptError={data.faults.some((f) => f.kind === "script_error")}
              onRefresh={refetch}
            />
          ) : (
            <>
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

              <Divider />

              <Section title="Resources">
                <ResourcesSection
                  appName={name!}
                  resources={data.resources}
                  onRefresh={refetch}
                />
              </Section>
            </>
          )}
        </Stack>
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
