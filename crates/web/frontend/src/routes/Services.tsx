import AddIcon from "@mui/icons-material/Add";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutlineOutlined";
import EditIcon from "@mui/icons-material/Edit";
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
  Typography,
} from "@mui/material";
import { useState } from "react";
import { Link } from "react-router-dom";
import {
  IconActionButton,
  OutlinedActionButton,
  SolidActionButton,
} from "../components/ActionButton";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useOiAction } from "../hooks/useOiAction";
import { useOiQuery } from "../hooks/useOi";
import {
  formatRemoteEndpoint,
  formatServiceTarget,
  looksLikeIpv4Literal,
  looksLikeIpv6Literal,
  looksLikeRemoteHost,
} from "../lib/services";
import type {
  AppService,
  DeclaredExternalService,
  ExportedService,
  ExternalServiceMapping,
  ServiceRef,
  SiteService,
  SiteServiceProtocol,
  SiteServiceResolverStatus,
} from "../lib/types";

const PROTOCOLS: SiteServiceProtocol[] = ["tcp", "udp", "http"];

/// Render a small inline status hint next to a site-service endpoint's
/// remote_host. IP literals are routed directly so they get no badge; DNS
/// names show "resolved", "resolving", or "failed" based on the daemon's
/// resolver-status snapshot.
function renderResolverBadge(
  host: string,
  status: SiteServiceResolverStatus | null | undefined,
): React.ReactElement | null {
  if (looksLikeIpv6Literal(host) || looksLikeIpv4Literal(host)) return null;
  const entry = status?.entries.find((e) => e.host === host);
  let label = "resolving";
  let color: "default" | "success" | "warning" | "error" = "default";
  if (entry) {
    if (entry.last_attempt_failed && entry.aaaa.length === 0 && entry.a.length === 0) {
      label = "failed";
      color = "error";
    } else if (entry.aaaa.length > 0 || entry.a.length > 0) {
      label = "resolved";
      color = "success";
    } else {
      label = "no records";
      color = "warning";
    }
  }
  return (
    <Chip
      label={label}
      size="small"
      variant="outlined"
      color={color === "default" ? undefined : color}
      sx={{ ml: 1, height: 18, fontSize: "0.65rem" }}
    />
  );
}

function ConfirmDeleteSiteServiceDialog({
  service,
  onCancel,
  onConfirm,
  loading,
}: {
  service: SiteService;
  onCancel: () => void;
  onConfirm: () => void;
  loading: boolean;
}) {
  return (
    <Dialog open onClose={loading ? undefined : onCancel} maxWidth="xs" fullWidth>
      <DialogTitle>Delete site service?</DialogTitle>
      <DialogContent>
        <Typography variant="body2" sx={{ mb: 2 }}>
          Delete site service{" "}
          <Box component="span" sx={{ fontFamily: "monospace" }}>
            {service.name}
          </Box>
          ? The OI will refuse this while any app has an external-service
          slot mapped here; unmap or remap first.
        </Typography>
      </DialogContent>
      <DialogActions>
        <Button onClick={onCancel} disabled={loading}>
          Cancel
        </Button>
        <SolidActionButton
          safety="dangerous"
          onClick={onConfirm}
          disabled={loading}
        >
          {loading ? "Deleting…" : "Delete"}
        </SolidActionButton>
      </DialogActions>
    </Dialog>
  );
}

function CreateSiteServiceDialog({
  open,
  onClose,
  onSuccess,
}: {
  open: boolean;
  onClose: () => void;
  onSuccess: () => void;
}) {
  const { execute, loading, error, clearError } = useOiAction();
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");

  const handleClose = () => {
    clearError();
    setName("");
    setDescription("");
    onClose();
  };

  const handleSubmit = async () => {
    const body: Record<string, unknown> = { name };
    if (description.trim()) body.description = description.trim();
    if ((await execute("/services/site/create", body)) === null) return;
    onSuccess();
    handleClose();
  };

  return (
    <Dialog open={open} onClose={handleClose} maxWidth="sm" fullWidth>
      <DialogTitle>New Site Service</DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 0.5 }}>
          {error && <OiErrorAlert error={error} />}
          <TextField
            label="Name"
            size="small"
            value={name}
            onChange={(e) => setName(e.target.value)}
            autoFocus
            slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
          />
          <TextField
            label="Description (optional)"
            size="small"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
          />
          <Typography variant="caption" sx={{ color: "text.secondary" }}>
            Backing endpoints are added separately. After creating, use
            "Add endpoint" on the service row.
          </Typography>
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={handleClose} disabled={loading}>
          Cancel
        </Button>
        <SolidActionButton
          safety="write"
          onClick={() => void handleSubmit()}
          disabled={loading || !name}
        >
          {loading ? "Creating…" : "Create"}
        </SolidActionButton>
      </DialogActions>
    </Dialog>
  );
}

function AddEndpointDialog({
  service,
  onClose,
  onSuccess,
}: {
  service: SiteService;
  onClose: () => void;
  onSuccess: () => void;
}) {
  const { execute, loading, error, clearError } = useOiAction();
  const [servicePort, setServicePort] = useState("");
  const [protocol, setProtocol] = useState<SiteServiceProtocol>("tcp");
  const [remoteHost, setRemoteHost] = useState("");
  const [remotePort, setRemotePort] = useState("");
  const [validationError, setValidationError] = useState<string | null>(null);

  const handleClose = () => {
    clearError();
    onClose();
  };

  const handleSubmit = async () => {
    setValidationError(null);
    const svcPort = Number(servicePort);
    const remPort = Number(remotePort);
    if (!Number.isInteger(svcPort) || svcPort < 1 || svcPort > 65535) {
      setValidationError("service port must be 1–65535");
      return;
    }
    if (!Number.isInteger(remPort) || remPort < 1 || remPort > 65535) {
      setValidationError("remote port must be 1–65535");
      return;
    }
    if (!looksLikeRemoteHost(remoteHost)) {
      setValidationError(
        "remote host must be an IP literal (IPv6/IPv4) or a valid DNS name",
      );
      return;
    }
    const result = await execute("/services/site/endpoint/add", {
      name: service.name,
      service_port: svcPort,
      protocol,
      remote_host: remoteHost,
      remote_port: remPort,
    });
    if (result === null) return;
    onSuccess();
    onClose();
  };

  return (
    <Dialog open onClose={loading ? undefined : handleClose} maxWidth="sm" fullWidth>
      <DialogTitle>
        Add endpoint to{" "}
        <Box component="span" sx={{ fontFamily: "monospace" }}>
          {service.name}
        </Box>
      </DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 0.5 }}>
          {error && <OiErrorAlert error={error} />}
          {validationError && <Alert severity="error">{validationError}</Alert>}
          <Stack direction="row" spacing={2}>
            <TextField
              label="Service port"
              size="small"
              value={servicePort}
              onChange={(e) => setServicePort(e.target.value)}
              sx={{ flex: 1 }}
              slotProps={{ htmlInput: { inputMode: "numeric" } }}
            />
            <FormControl size="small" sx={{ minWidth: 120 }}>
              <InputLabel>Protocol</InputLabel>
              <Select
                label="Protocol"
                value={protocol}
                onChange={(e) => setProtocol(e.target.value as SiteServiceProtocol)}
              >
                {PROTOCOLS.map((p) => (
                  <MenuItem key={p} value={p}>
                    {p}
                  </MenuItem>
                ))}
              </Select>
            </FormControl>
          </Stack>
          <Stack direction="row" spacing={2}>
            <TextField
              label="Remote host"
              size="small"
              value={remoteHost}
              onChange={(e) => setRemoteHost(e.target.value)}
              placeholder="2001:db8::1, 10.0.0.1, or db.example.com"
              sx={{ flex: 2 }}
              slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
            />
            <TextField
              label="Remote port"
              size="small"
              value={remotePort}
              onChange={(e) => setRemotePort(e.target.value)}
              sx={{ flex: 1 }}
              slotProps={{ htmlInput: { inputMode: "numeric" } }}
            />
          </Stack>
          <Typography variant="caption" sx={{ color: "text.secondary" }}>
            Service port and remote port may differ. IPv4 and A-only DNS
            backends route via NAT64; status is shown in the resolver
            cache.
          </Typography>
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={handleClose} disabled={loading}>
          Cancel
        </Button>
        <SolidActionButton
          safety="write"
          onClick={() => void handleSubmit()}
          disabled={loading || !servicePort || !remoteHost || !remotePort}
        >
          {loading ? "Adding…" : "Add endpoint"}
        </SolidActionButton>
      </DialogActions>
    </Dialog>
  );
}

function MapExternalServiceDialog({
  open,
  onClose,
  onSuccess,
  siteServices,
  declared,
  existing,
  prefill,
}: {
  open: boolean;
  onClose: () => void;
  onSuccess: () => void;
  siteServices: SiteService[];
  declared: DeclaredExternalService[];
  existing?: ExternalServiceMapping;
  prefill?: { app: string; name: string };
}) {
  const { execute, loading, error, clearError } = useOiAction();
  const { data: appServices } = useOiQuery<AppService[]>("/services/app/list", {});

  const isRemap = existing != null;
  const isFixed = isRemap || prefill != null;

  const initialApp = existing?.app ?? prefill?.app ?? "";
  const initialSlot = existing?.external_name ?? prefill?.name ?? "";

  const [slotKey, setSlotKey] = useState(
    initialApp && initialSlot ? `${initialApp}\0${initialSlot}` : "",
  );
  const [kind, setKind] = useState<"site" | "app">(
    existing?.target.kind ?? "site",
  );
  const [siteName, setSiteName] = useState(
    existing?.target.kind === "site" ? existing.target.name : "",
  );
  const [targetKey, setTargetKey] = useState(
    existing?.target.kind === "app"
      ? `${existing.target.app}\0${existing.target.service}`
      : "",
  );

  const app = isFixed ? initialApp : slotKey.split("\0")[0] ?? "";
  const slot = isFixed ? initialSlot : slotKey.split("\0")[1] ?? "";
  const [targetApp, targetService] = targetKey ? targetKey.split("\0") : ["", ""];

  const handleClose = () => {
    clearError();
    onClose();
  };

  const handleSubmit = async () => {
    const target: ServiceRef =
      kind === "site"
        ? { kind: "site", name: siteName }
        : { kind: "app", app: targetApp, service: targetService };
    const path = isRemap ? "/services/external/remap" : "/services/external/map";
    if ((await execute(path, { app, external_name: slot, target })) === null) return;
    onSuccess();
    onClose();
  };

  const canSubmit =
    !!app &&
    !!slot &&
    (kind === "site" ? !!siteName : !!targetApp && !!targetService);

  return (
    <Dialog open={open} onClose={loading ? undefined : handleClose} maxWidth="sm" fullWidth>
      <DialogTitle>
        {isRemap ? "Remap external service" : "Map external service"}
      </DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 0.5 }}>
          {error && <OiErrorAlert error={error} />}
          {isFixed ? (
            <TextField
              label="App / Slot"
              size="small"
              value={`${app} / ${slot}`}
              disabled
              slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
            />
          ) : declared.length > 0 ? (
            <FormControl size="small" fullWidth>
              <InputLabel>App / Slot</InputLabel>
              <Select
                label="App / Slot"
                value={slotKey}
                onChange={(e) => setSlotKey(e.target.value)}
                sx={{ fontFamily: "monospace" }}
                autoFocus
              >
                {declared.map((d) => (
                  <MenuItem
                    key={`${d.app}\0${d.name}`}
                    value={`${d.app}\0${d.name}`}
                    sx={{ fontFamily: "monospace" }}
                  >
                    {d.app}
                    <Typography
                      component="span"
                      sx={{ color: "text.secondary", mx: 0.5 }}
                    >
                      /
                    </Typography>
                    {d.name}
                  </MenuItem>
                ))}
              </Select>
            </FormControl>
          ) : (
            <Typography variant="body2" sx={{ color: "text.secondary" }}>
              No external service slots declared by any registered app.
            </Typography>
          )}
          <FormControl>
            <FormLabel>Target kind</FormLabel>
            <RadioGroup
              row
              value={kind}
              onChange={(e) => setKind(e.target.value as "site" | "app")}
            >
              <FormControlLabel
                value="site"
                control={<Radio size="small" />}
                label="Site service"
              />
              <FormControlLabel
                value="app"
                control={<Radio size="small" />}
                label="App service"
              />
            </RadioGroup>
          </FormControl>
          {kind === "site" ? (
            siteServices.length > 0 ? (
              <FormControl size="small">
                <InputLabel>Site service</InputLabel>
                <Select
                  label="Site service"
                  value={siteName}
                  onChange={(e) => setSiteName(e.target.value)}
                  sx={{ fontFamily: "monospace" }}
                >
                  {siteServices.map((s) => (
                    <MenuItem
                      key={s.name}
                      value={s.name}
                      sx={{ fontFamily: "monospace" }}
                    >
                      {s.name}
                    </MenuItem>
                  ))}
                </Select>
              </FormControl>
            ) : (
              <TextField
                label="Site service name"
                size="small"
                value={siteName}
                onChange={(e) => setSiteName(e.target.value)}
                helperText="No site services registered yet."
                slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
              />
            )
          ) : (appServices ?? []).length > 0 ? (
            <FormControl size="small" fullWidth>
              <InputLabel>Target app / service</InputLabel>
              <Select
                label="Target app / service"
                value={targetKey}
                onChange={(e) => setTargetKey(e.target.value)}
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
              No app services available.
            </Typography>
          )}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={handleClose} disabled={loading}>
          Cancel
        </Button>
        <SolidActionButton
          safety="write"
          onClick={() => void handleSubmit()}
          disabled={loading || !canSubmit}
        >
          {loading ? (isRemap ? "Remapping…" : "Mapping…") : isRemap ? "Remap" : "Map"}
        </SolidActionButton>
      </DialogActions>
    </Dialog>
  );
}

export default function Services() {
  const {
    data: siteSvcs,
    loading: siteLoading,
    error: siteError,
    refetch: refetchSite,
  } = useOiQuery<SiteService[]>("/services/site/list", {});
  const {
    data: exportedSvcs,
    loading: exportedLoading,
    error: exportedError,
    refetch: refetchExported,
  } = useOiQuery<ExportedService[]>("/services/exported/list", {});
  const {
    data: mappings,
    loading: mappingsLoading,
    error: mappingsError,
    refetch: refetchMappings,
  } = useOiQuery<ExternalServiceMapping[]>("/services/external/list", {});
  const {
    data: declared,
    loading: declaredLoading,
    error: declaredError,
    refetch: refetchDeclared,
  } = useOiQuery<DeclaredExternalService[]>("/services/external/declared", {});
  const { data: resolverStatus } = useOiQuery<SiteServiceResolverStatus>(
    "/services/site/resolver-status",
    {},
  );

  const { execute, error: actionError } = useOiAction();

  const [createOpen, setCreateOpen] = useState(false);
  const [mapOpen, setMapOpen] = useState(false);
  const [addEndpointTarget, setAddEndpointTarget] = useState<SiteService | null>(
    null,
  );
  const [deleteTarget, setDeleteTarget] = useState<SiteService | null>(null);
  const [remapTarget, setRemapTarget] = useState<ExternalServiceMapping | null>(
    null,
  );
  const [prefillTarget, setPrefillTarget] = useState<
    { app: string; name: string } | null
  >(null);
  const [deleteBusy, setDeleteBusy] = useState(false);

  const refreshAll = () => {
    refetchSite();
    refetchExported();
    refetchMappings();
    refetchDeclared();
  };

  const confirmDelete = async () => {
    if (!deleteTarget) return;
    setDeleteBusy(true);
    try {
      if ((await execute("/services/site/delete", { name: deleteTarget.name })) !== null) {
        refetchSite();
        setDeleteTarget(null);
      }
    } finally {
      setDeleteBusy(false);
    }
  };

  const removeEndpoint = async (
    svc: SiteService,
    service_port: number,
    protocol: SiteServiceProtocol,
    remote_host: string,
    remote_port: number,
  ) => {
    const result = await execute("/services/site/endpoint/remove", {
      name: svc.name,
      service_port,
      protocol,
      remote_host,
      remote_port,
    });
    if (result === null) return;
    refetchSite();
  };

  const unmap = async (app: string, external_name: string) => {
    if ((await execute("/services/external/unmap", { app, external_name })) === null) return;
    refetchMappings();
  };

  const anyLoading =
    siteLoading || exportedLoading || mappingsLoading || declaredLoading;

  return (
    <Box sx={{ p: 3, maxWidth: 900, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          Services
        </Typography>
        <IconActionButton
          safety="read"
          tooltip="Refresh"
          onClick={refreshAll}
          disabled={anyLoading}
        >
          <RefreshIcon />
        </IconActionButton>
      </Box>
      {actionError && (
        <Alert severity="error" sx={{ mb: 2 }}>
          {actionError.message}
        </Alert>
      )}
      <Stack spacing={4}>
        {/* Site Services */}
        <Box>
          <Box sx={{ display: "flex", alignItems: "center", mb: 1, gap: 1 }}>
            <Typography variant="subtitle1" sx={{ fontWeight: 600, flexGrow: 1 }}>
              Site Services
            </Typography>
            <OutlinedActionButton
              safety="write"
              size="small"
              startIcon={<AddIcon />}
              onClick={() => setCreateOpen(true)}
            >
              New
            </OutlinedActionButton>
          </Box>
          {siteError && <OiErrorAlert error={siteError} />}
          {siteLoading && !siteSvcs && <CircularProgress size={20} />}
          {siteSvcs &&
            (siteSvcs.length === 0 ? (
              <Typography variant="body2" sx={{ color: "text.secondary" }}>
                No site services.
              </Typography>
            ) : (
              <Stack spacing={1}>
                {siteSvcs.map((svc) => (
                  <Paper key={svc.name} variant="outlined" sx={{ p: 2 }}>
                    <Box
                      sx={{
                        display: "flex",
                        alignItems: "center",
                        gap: 1,
                        mb: 1,
                      }}
                    >
                      <Typography sx={{ fontFamily: "monospace", fontWeight: 500 }}>
                        {svc.name}
                      </Typography>
                      {svc.description && (
                        <Typography
                          variant="caption"
                          sx={{ color: "text.secondary" }}
                        >
                          — {svc.description}
                        </Typography>
                      )}
                      <Box sx={{ flexGrow: 1 }} />
                      <OutlinedActionButton
                        safety="write"
                        tooltip="Add endpoint"
                        size="small"
                        startIcon={<AddIcon />}
                        onClick={() => setAddEndpointTarget(svc)}
                      >
                        Add endpoint
                      </OutlinedActionButton>
                      <IconActionButton
                        safety="dangerous"
                        tooltip="Delete"
                        onClick={() => setDeleteTarget(svc)}
                      >
                        <DeleteOutlineIcon sx={{ fontSize: 16 }} />
                      </IconActionButton>
                    </Box>
                    {svc.endpoints.length === 0 ? (
                      <Typography
                        variant="body2"
                        sx={{ color: "text.secondary" }}
                      >
                        No endpoints yet — service will blackhole traffic until
                        at least one is added.
                      </Typography>
                    ) : (
                      <Table size="small">
                        <TableHead>
                          <TableRow>
                            <TableCell width={90}>Service port</TableCell>
                            <TableCell width={70}>Protocol</TableCell>
                            <TableCell>Remote</TableCell>
                            <TableCell width={50} />
                          </TableRow>
                        </TableHead>
                        <TableBody>
                          {svc.endpoints.map((ep) => (
                            <TableRow
                              key={`${ep.service_port}-${ep.protocol}-${ep.remote_host}-${ep.remote_port}`}
                            >
                              <TableCell sx={{ fontFamily: "monospace" }}>
                                {ep.service_port}
                              </TableCell>
                              <TableCell>
                                <Chip
                                  label={ep.protocol}
                                  size="small"
                                  variant="outlined"
                                />
                              </TableCell>
                              <TableCell sx={{ fontFamily: "monospace" }}>
                                {formatRemoteEndpoint(
                                  ep.remote_host,
                                  ep.remote_port,
                                )}
                                {renderResolverBadge(
                                  ep.remote_host,
                                  resolverStatus,
                                )}
                              </TableCell>
                              <TableCell align="right" sx={{ px: 0.5 }}>
                                <IconActionButton
                                  safety="dangerous"
                                  tooltip="Remove"
                                  onClick={() =>
                                    void removeEndpoint(
                                      svc,
                                      ep.service_port,
                                      ep.protocol,
                                      ep.remote_host,
                                      ep.remote_port,
                                    )
                                  }
                                >
                                  <DeleteOutlineIcon sx={{ fontSize: 14 }} />
                                </IconActionButton>
                              </TableCell>
                            </TableRow>
                          ))}
                        </TableBody>
                      </Table>
                    )}
                  </Paper>
                ))}
              </Stack>
            ))}
        </Box>

        <Divider />

        {/* App Exports */}
        <Box>
          <Typography variant="subtitle1" sx={{ fontWeight: 600, mb: 1 }}>
            App Exports
          </Typography>
          {exportedError && <OiErrorAlert error={exportedError} />}
          {exportedLoading && !exportedSvcs && <CircularProgress size={20} />}
          {exportedSvcs &&
            (exportedSvcs.length === 0 ? (
              <Typography variant="body2" sx={{ color: "text.secondary" }}>
                No exported services.
              </Typography>
            ) : (
              <TableContainer component={Paper} variant="outlined">
                <Table size="small">
                  <TableHead>
                    <TableRow>
                      <TableCell>App</TableCell>
                      <TableCell>Service</TableCell>
                      <TableCell width={70}>HTTP</TableCell>
                      <TableCell>Description</TableCell>
                    </TableRow>
                  </TableHead>
                  <TableBody>
                    {exportedSvcs.map((s) => (
                      <TableRow key={`${s.app}/${s.service_name}`}>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          <Link to={`/apps/${s.app}`}>{s.app}</Link>
                        </TableCell>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          {s.service_name}
                        </TableCell>
                        <TableCell>
                          {s.http && (
                            <Chip label="http" size="small" variant="outlined" />
                          )}
                        </TableCell>
                        <TableCell sx={{ color: "text.secondary" }}>
                          {s.description ?? "—"}
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </TableContainer>
            ))}
        </Box>

        <Divider />

        {/* External Service Requests */}
        <Box>
          <Box sx={{ display: "flex", alignItems: "center", mb: 1, gap: 1 }}>
            <Typography variant="subtitle1" sx={{ fontWeight: 600, flexGrow: 1 }}>
              External Service Requests
            </Typography>
            <OutlinedActionButton
              safety="write"
              size="small"
              startIcon={<AddIcon />}
              onClick={() => setMapOpen(true)}
            >
              Map
            </OutlinedActionButton>
          </Box>
          {declaredError && <OiErrorAlert error={declaredError} />}
          {mappingsError && <OiErrorAlert error={mappingsError} />}
          {(declaredLoading || mappingsLoading) && !declared && (
            <CircularProgress size={20} />
          )}
          {declared && (
            declared.length === 0 ? (
              <Typography variant="body2" sx={{ color: "text.secondary" }}>
                No external service slots declared across registered apps.
              </Typography>
            ) : (
              <TableContainer component={Paper} variant="outlined">
                <Table size="small">
                  <TableHead>
                    <TableRow>
                      <TableCell>App</TableCell>
                      <TableCell>Slot</TableCell>
                      <TableCell>Target</TableCell>
                      <TableCell width={80} />
                    </TableRow>
                  </TableHead>
                  <TableBody>
                    {declared.map((d) => {
                      const mapping = mappings?.find(
                        (m) => m.app === d.app && m.external_name === d.name,
                      );
                      return (
                        <TableRow key={`${d.app}/${d.name}`}>
                          <TableCell sx={{ fontFamily: "monospace" }}>
                            <Link to={`/apps/${d.app}`}>{d.app}</Link>
                          </TableCell>
                          <TableCell sx={{ fontFamily: "monospace" }}>
                            {d.name}
                          </TableCell>
                          <TableCell sx={{ fontFamily: "monospace" }}>
                            {mapping ? (
                              formatServiceTarget(mapping.target)
                            ) : (
                              <Typography
                                variant="caption"
                                sx={{ color: "warning.main" }}
                              >
                                unmapped
                              </Typography>
                            )}
                          </TableCell>
                          <TableCell
                            align="right"
                            sx={{ px: 0.5, whiteSpace: "nowrap" }}
                          >
                            {mapping ? (
                              <>
                                <IconActionButton
                                  safety="write"
                                  tooltip="Remap"
                                  onClick={() => setRemapTarget(mapping)}
                                >
                                  <EditIcon sx={{ fontSize: 16 }} />
                                </IconActionButton>
                                <IconActionButton
                                  safety="write"
                                  tooltip="Unmap"
                                  onClick={() => void unmap(d.app, d.name)}
                                >
                                  <LinkOffIcon sx={{ fontSize: 16 }} />
                                </IconActionButton>
                              </>
                            ) : (
                              <OutlinedActionButton
                                safety="write"
                                size="small"
                                onClick={() =>
                                  setPrefillTarget({
                                    app: d.app,
                                    name: d.name,
                                  })
                                }
                              >
                                Map
                              </OutlinedActionButton>
                            )}
                          </TableCell>
                        </TableRow>
                      );
                    })}
                  </TableBody>
                </Table>
              </TableContainer>
            )
          )}
        </Box>
      </Stack>
      <CreateSiteServiceDialog
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        onSuccess={refetchSite}
      />
      {addEndpointTarget && (
        <AddEndpointDialog
          service={addEndpointTarget}
          onClose={() => setAddEndpointTarget(null)}
          onSuccess={refetchSite}
        />
      )}
      {deleteTarget && (
        <ConfirmDeleteSiteServiceDialog
          service={deleteTarget}
          onCancel={() => setDeleteTarget(null)}
          onConfirm={() => void confirmDelete()}
          loading={deleteBusy}
        />
      )}
      {(mapOpen || remapTarget != null || prefillTarget != null) && (
        <MapExternalServiceDialog
          key={
            remapTarget
              ? `remap:${remapTarget.app}/${remapTarget.external_name}`
              : prefillTarget
                ? `prefill:${prefillTarget.app}/${prefillTarget.name}`
                : "new"
          }
          open={mapOpen || remapTarget != null || prefillTarget != null}
          onClose={() => {
            setMapOpen(false);
            setRemapTarget(null);
            setPrefillTarget(null);
          }}
          onSuccess={() => {
            refetchMappings();
            refetchDeclared();
            setMapOpen(false);
            setRemapTarget(null);
            setPrefillTarget(null);
          }}
          siteServices={siteSvcs ?? []}
          declared={declared ?? []}
          existing={remapTarget ?? undefined}
          prefill={prefillTarget ?? undefined}
        />
      )}
    </Box>
  );
}
