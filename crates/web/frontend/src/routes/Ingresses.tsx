// w[impl routes.ingresses]
import AddIcon from "@mui/icons-material/Add";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutlineOutlined";
import LinkOffIcon from "@mui/icons-material/LinkOff";
import RefreshIcon from "@mui/icons-material/Refresh";
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
  FormControl,
  FormControlLabel,
  FormLabel,
  IconButton,
  InputLabel,
  MenuItem,
  Paper,
  Radio,
  RadioGroup,
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
import { useState } from "react";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useGuard } from "../components/SafetyModeProvider";
import { useOiAction } from "../hooks/useOiAction";
import { useOiQuery } from "../hooks/useOi";
import type {
  AppService,
  AttachmentProtocol,
  SiteIngress,
  SiteIngressAttachment,
  SiteIngressDiscoveryStatus,
  SiteIngressTlsProvider,
} from "../lib/types";

const PROTOCOLS: AttachmentProtocol[] = ["tcp", "udp", "http", "http2"];
const HTTP_PROTOCOLS: AttachmentProtocol[] = ["http", "http2"];
const TLS_PROVIDERS: SiteIngressTlsProvider[] = ["acme", "internal", "none"];
const REDIRECT_CODES = [301, 302, 307, 308] as const;

function tlsLabel(provider: SiteIngressTlsProvider): string {
  switch (provider) {
    case "acme":
      return "ACME";
    case "tailscale":
      return "Tailscale";
    case "internal":
      return "Internal CA";
    case "none":
      return "No TLS";
  }
}

function describeAttachment(att: SiteIngressAttachment): string {
  if (att.target_kind === "forward") {
    return `${att.target_app}/${att.target_service}`;
  }
  return `↦ ${att.redirect_url} (${att.redirect_code})`;
}

function CreateSiteIngressDialog({
  onCancel,
  onCreated,
}: {
  onCancel: () => void;
  onCreated: () => void;
}) {
  const [name, setName] = useState("");
  const [hostname, setHostname] = useState("");
  const [description, setDescription] = useState("");
  const [tlsProvider, setTlsProvider] = useState<SiteIngressTlsProvider>("acme");
  const guard = useGuard("write");
  const { execute, loading, error } = useOiAction();
  const onSubmit = async () => {
    const params: Record<string, unknown> = {
      name,
      hostname,
      tls_provider: tlsProvider,
    };
    if (description.trim()) params.description = description.trim();
    try {
      await execute("/ingresses/site/create", params);
      onCreated();
    } catch {
      /* error surfaced via `error` state */
    }
  };
  const valid = name.trim() !== "" && hostname.trim() !== "";
  return (
    <Dialog open onClose={loading ? undefined : onCancel} maxWidth="sm" fullWidth>
      <DialogTitle>Create site ingress</DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 1 }}>
          <TextField
            label="Name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            helperText="3–63 chars, ASCII alphanumeric + hyphen, no leading underscore"
            size="small"
            autoFocus
          />
          <TextField
            label="Hostname"
            value={hostname}
            onChange={(e) => setHostname(e.target.value)}
            helperText="Public DNS name (no wildcards), e.g. old.example.com"
            size="small"
          />
          <TextField
            label="Description (optional)"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            size="small"
            multiline
            minRows={1}
            maxRows={3}
          />
          <FormControl size="small">
            <InputLabel id="tls-provider-label">TLS</InputLabel>
            <Select
              labelId="tls-provider-label"
              label="TLS"
              value={tlsProvider}
              onChange={(e) => setTlsProvider(e.target.value as SiteIngressTlsProvider)}
            >
              {TLS_PROVIDERS.map((p) => (
                <MenuItem key={p} value={p}>
                  {tlsLabel(p)}
                </MenuItem>
              ))}
            </Select>
          </FormControl>
          {error && <OiErrorAlert error={error} />}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onCancel} disabled={loading}>
          Cancel
        </Button>
        <Tooltip title={guard.reason ?? ""}>
          <span>
            <Button
              variant="contained"
              onClick={onSubmit}
              disabled={loading || !valid || !guard.allowed}
            >
              {loading ? "Creating…" : "Create"}
            </Button>
          </span>
        </Tooltip>
      </DialogActions>
    </Dialog>
  );
}

function ConfirmDeleteSiteIngressDialog({
  ingress,
  onCancel,
  onConfirm,
  loading,
}: {
  ingress: SiteIngress;
  onCancel: () => void;
  onConfirm: () => void;
  loading: boolean;
}) {
  const guard = useGuard("dangerous");
  return (
    <Dialog open onClose={loading ? undefined : onCancel} maxWidth="xs" fullWidth>
      <DialogTitle>Delete site ingress?</DialogTitle>
      <DialogContent>
        <Typography variant="body2" sx={{ mb: 2 }}>
          Delete site ingress{" "}
          <Box component="span" sx={{ fontFamily: "monospace" }}>
            {ingress.name}
          </Box>
          ? All attachments are removed. Discovered ingresses are managed by
          the daemon and cannot be deleted while their source is active.
        </Typography>
      </DialogContent>
      <DialogActions>
        <Button onClick={onCancel} disabled={loading}>
          Cancel
        </Button>
        <Tooltip title={guard.reason ?? ""}>
          <span>
            <Button
              variant="contained"
              color="error"
              onClick={onConfirm}
              disabled={loading || !guard.allowed}
            >
              {loading ? "Deleting…" : "Delete"}
            </Button>
          </span>
        </Tooltip>
      </DialogActions>
    </Dialog>
  );
}

function AttachDialog({
  ingress,
  onCancel,
  onAttached,
}: {
  ingress: SiteIngress;
  onCancel: () => void;
  onAttached: () => void;
}) {
  const [kind, setKind] = useState<"forward" | "redirect">("forward");
  const [port, setPort] = useState("443");
  const [protocol, setProtocol] = useState<AttachmentProtocol>("http");
  const [target, setTarget] = useState("");
  const [redirectUrl, setRedirectUrl] = useState("");
  const [redirectCode, setRedirectCode] = useState<number>(307);
  const [preservePath, setPreservePath] = useState(true);
  const guard = useGuard("write");
  const { execute, loading, error } = useOiAction();
  const { data: appServices } = useOiQuery<AppService[]>("/services/app/list", {});
  const [targetApp, targetService] = target ? target.split("\0") : ["", ""];
  const onSubmit = async () => {
    const portNum = Number.parseInt(port, 10);
    if (!Number.isFinite(portNum) || portNum < 1 || portNum > 65535) return;
    try {
      if (kind === "forward") {
        await execute("/ingresses/site/attach/forward", {
          name: ingress.name,
          port: portNum,
          protocol,
          target_app: targetApp,
          target_service: targetService,
        });
      } else {
        await execute("/ingresses/site/attach/redirect", {
          name: ingress.name,
          port: portNum,
          protocol,
          redirect_url: redirectUrl,
          redirect_code: redirectCode,
          preserve_path: preservePath,
        });
      }
      onAttached();
    } catch {
      /* error surfaced via `error` state */
    }
  };
  const portValid = (() => {
    const n = Number.parseInt(port, 10);
    return Number.isFinite(n) && n >= 1 && n <= 65535;
  })();
  const valid =
    portValid &&
    (kind === "forward"
      ? targetApp.trim() !== "" && targetService.trim() !== ""
      : redirectUrl.trim().startsWith("http://") || redirectUrl.trim().startsWith("https://"));
  return (
    <Dialog open onClose={loading ? undefined : onCancel} maxWidth="sm" fullWidth>
      <DialogTitle>Attach to {ingress.name}</DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 1 }}>
          <FormControl>
            <FormLabel>Target kind</FormLabel>
            <RadioGroup
              row
              value={kind}
              onChange={(_, v) => setKind(v as "forward" | "redirect")}
            >
              <FormControlLabel value="forward" control={<Radio />} label="Forward to app" />
              <FormControlLabel value="redirect" control={<Radio />} label="Redirect to URL" />
            </RadioGroup>
          </FormControl>
          <Stack direction="row" spacing={2}>
            <TextField
              label="Port"
              size="small"
              value={port}
              onChange={(e) => setPort(e.target.value)}
              sx={{ width: 120 }}
            />
            <FormControl size="small" sx={{ minWidth: 120 }}>
              <InputLabel id="att-protocol">Protocol</InputLabel>
              <Select
                labelId="att-protocol"
                label="Protocol"
                value={protocol}
                onChange={(e) => setProtocol(e.target.value as AttachmentProtocol)}
              >
                {(kind === "forward" ? PROTOCOLS : HTTP_PROTOCOLS).map((p) => (
                  <MenuItem key={p} value={p}>
                    {p}
                  </MenuItem>
                ))}
              </Select>
            </FormControl>
          </Stack>
          {kind === "forward" ? (
            (appServices ?? []).length > 0 ? (
              <FormControl size="small" fullWidth>
                <InputLabel id="att-target">Target app / service</InputLabel>
                <Select
                  labelId="att-target"
                  label="Target app / service"
                  value={target}
                  onChange={(e) => setTarget(e.target.value)}
                  sx={{ fontFamily: "monospace" }}
                >
                  {(appServices ?? []).map((s) => (
                    <MenuItem
                      key={`${s.app}\0${s.service_name}`}
                      value={`${s.app}\0${s.service_name}`}
                      sx={{ fontFamily: "monospace" }}
                    >
                      {s.app}
                      <Typography
                        component="span"
                        sx={{ color: "text.secondary", mx: 0.5 }}
                      >
                        /
                      </Typography>
                      {s.service_name}
                      {s.http && (
                        <Typography
                          component="span"
                          variant="caption"
                          sx={{ color: "text.secondary", ml: 1 }}
                        >
                          http
                        </Typography>
                      )}
                      {!s.exported && (
                        <Typography
                          component="span"
                          variant="caption"
                          sx={{ color: "warning.main", ml: 1 }}
                        >
                          not exported
                        </Typography>
                      )}
                    </MenuItem>
                  ))}
                </Select>
              </FormControl>
            ) : (
              <Typography variant="body2" sx={{ color: "text.secondary" }}>
                No app services available. Register an app that exports a
                service before attaching a forward.
              </Typography>
            )
          ) : (
            <Stack spacing={2}>
              <TextField
                label="Redirect URL"
                size="small"
                value={redirectUrl}
                onChange={(e) => setRedirectUrl(e.target.value)}
                helperText="Must start with http:// or https://"
                fullWidth
              />
              <Box sx={{ display: "flex", flexDirection: "row", gap: 2, alignItems: "center" }}>
                <FormControl size="small" sx={{ minWidth: 100 }}>
                  <InputLabel id="redirect-code">Code</InputLabel>
                  <Select
                    labelId="redirect-code"
                    label="Code"
                    value={redirectCode}
                    onChange={(e) => setRedirectCode(Number(e.target.value))}
                  >
                    {REDIRECT_CODES.map((c) => (
                      <MenuItem key={c} value={c}>
                        {c}
                      </MenuItem>
                    ))}
                  </Select>
                </FormControl>
                <FormControlLabel
                  control={
                    <Radio
                      checked={preservePath}
                      onClick={() => setPreservePath(!preservePath)}
                    />
                  }
                  label="Preserve request path"
                />
              </Box>
            </Stack>
          )}
          {error && <OiErrorAlert error={error} />}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onCancel} disabled={loading}>
          Cancel
        </Button>
        <Tooltip title={guard.reason ?? ""}>
          <span>
            <Button
              variant="contained"
              onClick={onSubmit}
              disabled={loading || !valid || !guard.allowed}
            >
              {loading ? "Attaching…" : "Attach"}
            </Button>
          </span>
        </Tooltip>
      </DialogActions>
    </Dialog>
  );
}

function IngressRow({
  ingress,
  onAttach,
  onDetach,
  onDelete,
}: {
  ingress: SiteIngress;
  onAttach: (i: SiteIngress) => void;
  onDetach: (i: SiteIngress, att: SiteIngressAttachment) => void;
  onDelete: (i: SiteIngress) => void;
}) {
  const writeGuard = useGuard("write");
  const dangerGuard = useGuard("dangerous");
  const isDiscovered = ingress.source === "discovered";
  return (
    <TableRow>
      <TableCell sx={{ fontFamily: "monospace" }}>{ingress.name}</TableCell>
      <TableCell sx={{ fontFamily: "monospace" }}>{ingress.hostname}</TableCell>
      <TableCell>
        <Stack direction="row" spacing={0.5}>
          <Chip
            size="small"
            label={isDiscovered ? `Discovered · ${ingress.discovered_provider}` : "Manual"}
            color={isDiscovered ? "info" : "default"}
            variant={isDiscovered ? "outlined" : "filled"}
          />
          {ingress.stale && (
            <Chip size="small" label="Stale" color="warning" />
          )}
        </Stack>
      </TableCell>
      <TableCell>
        <Chip size="small" label={tlsLabel(ingress.tls_provider)} variant="outlined" />
      </TableCell>
      <TableCell>
        {ingress.attachments.length === 0 ? (
          <Typography variant="caption" color="text.secondary">
            (no attachments)
          </Typography>
        ) : (
          <Stack spacing={0.5}>
            {ingress.attachments.map((att) => (
              <Box
                key={`${att.port}-${att.protocol}`}
                sx={{ display: "flex", flexDirection: "row", gap: 1, alignItems: "center" }}
              >
                <Chip
                  size="small"
                  label={`${att.port}/${att.protocol}`}
                  variant="outlined"
                  sx={{ fontFamily: "monospace" }}
                />
                <Box component="span" sx={{ fontFamily: "monospace", fontSize: "0.85em" }}>
                  {describeAttachment(att)}
                </Box>
                <Tooltip title={dangerGuard.reason ?? "Detach"}>
                  <span>
                    <IconButton
                      size="small"
                      onClick={() => onDetach(ingress, att)}
                      disabled={!dangerGuard.allowed}
                    >
                      <LinkOffIcon fontSize="small" />
                    </IconButton>
                  </span>
                </Tooltip>
              </Box>
            ))}
          </Stack>
        )}
      </TableCell>
      <TableCell align="right">
        <Box sx={{ display: "flex", flexDirection: "row", gap: 0.5, justifyContent: "flex-end" }}>
          <Tooltip title={writeGuard.reason ?? "Attach forward or redirect"}>
            <span>
              <IconButton
                size="small"
                onClick={() => onAttach(ingress)}
                disabled={!writeGuard.allowed}
              >
                <AddIcon fontSize="small" />
              </IconButton>
            </span>
          </Tooltip>
          <Tooltip
            title={
              isDiscovered
                ? "Discovered ingresses are managed by the provider and cannot be deleted here"
                : (dangerGuard.reason ?? "Delete this manual site ingress")
            }
          >
            <span>
              <IconButton
                size="small"
                onClick={() => onDelete(ingress)}
                disabled={isDiscovered || !dangerGuard.allowed}
              >
                <DeleteOutlineIcon fontSize="small" />
              </IconButton>
            </span>
          </Tooltip>
        </Box>
      </TableCell>
    </TableRow>
  );
}

export default function Ingresses() {
  const list = useOiQuery<SiteIngress[]>("/ingresses/site/list", {});
  const discovery = useOiQuery<SiteIngressDiscoveryStatus>(
    "/ingresses/site/discovery/status",
    {},
  );
  const guard = useGuard("write");
  const { execute: executeDetach, error: detachError } = useOiAction();
  const {
    execute: executeRemove,
    loading: removeLoading,
    error: removeError,
  } = useOiAction();
  const [createOpen, setCreateOpen] = useState(false);
  const [attachTarget, setAttachTarget] = useState<SiteIngress | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<SiteIngress | null>(null);

  const refresh = () => {
    list.refetch();
    discovery.refetch();
  };

  const onDetach = async (ingress: SiteIngress, att: SiteIngressAttachment) => {
    try {
      await executeDetach("/ingresses/site/detach", {
        name: ingress.name,
        port: att.port,
        protocol: att.protocol,
      });
      refresh();
    } catch {
      /* error surfaced via `detachError` */
    }
  };

  const onConfirmDelete = async () => {
    if (!deleteTarget) return;
    try {
      await executeRemove("/ingresses/site/delete", { name: deleteTarget.name });
      setDeleteTarget(null);
      refresh();
    } catch {
      /* error surfaced via `removeError` */
    }
  };

  const ingresses = list.data ?? [];
  const manual = ingresses.filter((i) => i.source === "manual");
  const discovered = ingresses.filter((i) => i.source === "discovered");
  const tailscaleStaleOnly = discovery.data?.providers
    .find((p) => p.name === "tailscale")
    ?.ingresses.find((e) => e.stale);

  return (
    <Box sx={{ p: 3, maxWidth: 1400, mx: "auto" }}>
      <Box
        sx={{
          display: "flex",
          flexDirection: "row",
          alignItems: "center",
          gap: 1,
          mb: 2,
        }}
      >
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          Site ingresses
        </Typography>
        <IconButton onClick={refresh} aria-label="Refresh">
          <RefreshIcon />
        </IconButton>
        <Tooltip title={guard.reason ?? ""}>
          <span>
            <Button
              variant="contained"
              startIcon={<AddIcon />}
              onClick={() => setCreateOpen(true)}
              disabled={!guard.allowed}
            >
              New ingress
            </Button>
          </span>
        </Tooltip>
      </Box>

      {tailscaleStaleOnly && (
        <Alert severity="warning" sx={{ mb: 2 }}>
          Tailscale discovery is currently unhealthy (logged out or
          unreachable). Existing attachments are preserved and will resume
          serving once tailscaled comes back.
        </Alert>
      )}
      {detachError && (
        <Box sx={{ mb: 2 }}>
          <OiErrorAlert error={detachError} />
        </Box>
      )}
      {removeError && (
        <Box sx={{ mb: 2 }}>
          <OiErrorAlert error={removeError} />
        </Box>
      )}

      {list.loading ? (
        <CircularProgress />
      ) : (
        <Stack spacing={3}>
          <Section
            title="Manual"
            description="Operator-defined ingresses, e.g. for URL migration redirects."
            ingresses={manual}
            onAttach={setAttachTarget}
            onDetach={onDetach}
            onDelete={setDeleteTarget}
          />
          <Divider />
          <Section
            title="Discovered"
            description="Auto-managed by a provider (currently Tailscale). Operators can attach apps or redirects but cannot delete these entries."
            ingresses={discovered}
            onAttach={setAttachTarget}
            onDetach={onDetach}
            onDelete={setDeleteTarget}
          />
        </Stack>
      )}

      {createOpen && (
        <CreateSiteIngressDialog
          onCancel={() => setCreateOpen(false)}
          onCreated={() => {
            setCreateOpen(false);
            refresh();
          }}
        />
      )}
      {attachTarget && (
        <AttachDialog
          ingress={attachTarget}
          onCancel={() => setAttachTarget(null)}
          onAttached={() => {
            setAttachTarget(null);
            refresh();
          }}
        />
      )}
      {deleteTarget && (
        <ConfirmDeleteSiteIngressDialog
          ingress={deleteTarget}
          onCancel={() => setDeleteTarget(null)}
          onConfirm={onConfirmDelete}
          loading={removeLoading}
        />
      )}
    </Box>
  );
}

function Section({
  title,
  description,
  ingresses,
  onAttach,
  onDetach,
  onDelete,
}: {
  title: string;
  description: string;
  ingresses: SiteIngress[];
  onAttach: (i: SiteIngress) => void;
  onDetach: (i: SiteIngress, att: SiteIngressAttachment) => void;
  onDelete: (i: SiteIngress) => void;
}) {
  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Typography variant="subtitle1" sx={{ fontWeight: 600 }}>
        {title}
      </Typography>
      <Typography variant="caption" color="text.secondary" sx={{ display: "block", mb: 1 }}>
        {description}
      </Typography>
      {ingresses.length === 0 ? (
        <Typography variant="body2" color="text.secondary" sx={{ py: 1 }}>
          (none)
        </Typography>
      ) : (
        <TableContainer>
          <Table size="small">
            <TableHead>
              <TableRow>
                <TableCell>Name</TableCell>
                <TableCell>Hostname</TableCell>
                <TableCell>Source</TableCell>
                <TableCell>TLS</TableCell>
                <TableCell>Attachments</TableCell>
                <TableCell align="right" sx={{ width: 100 }}>
                  Actions
                </TableCell>
              </TableRow>
            </TableHead>
            <TableBody>
              {ingresses.map((i) => (
                <IngressRow
                  key={i.name}
                  ingress={i}
                  onAttach={onAttach}
                  onDetach={onDetach}
                  onDelete={onDelete}
                />
              ))}
            </TableBody>
          </Table>
        </TableContainer>
      )}
    </Paper>
  );
}
