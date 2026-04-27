import AddIcon from "@mui/icons-material/Add";
import BoltIcon from "@mui/icons-material/Bolt";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutlineOutlined";
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
  IconButton,
  MenuItem,
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
import { useMemo, useState } from "react";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useGuard } from "../components/SafetyModeProvider";
import type { OiQueryError } from "../hooks/useOi";
import { useOiQuery } from "../hooks/useOi";
import { useOiAction } from "../hooks/useOiAction";
import type {
  TlsCertificate,
  TlsCertificatesResponse,
  TlsDnsProvider,
  TlsDnsProvidersResponse,
  TlsPoliciesResponse,
  TlsPolicy,
  TlsSettings,
} from "../lib/types";

// 14-day expiring window matches the runtime's tls.fault.expiring threshold.
const EXPIRING_WINDOW_S = 14 * 24 * 60 * 60;

function formatTime(unix: number | null): string {
  if (!unix) return "—";
  return new Date(unix * 1000).toLocaleString();
}

function expiryChip(notAfter: number | null) {
  if (!notAfter) return null;
  const remaining = notAfter - Math.floor(Date.now() / 1000);
  if (remaining <= 0) {
    return <Chip label="expired" size="small" color="error" />;
  }
  if (remaining < EXPIRING_WINDOW_S) {
    const days = Math.max(1, Math.ceil(remaining / 86400));
    return (
      <Chip
        label={`expires in ${days}d`}
        size="small"
        color="warning"
        variant="outlined"
      />
    );
  }
  return null;
}

// w[impl routes.certificates]
export default function Certificates() {
  const {
    data: providers,
    loading: providersLoading,
    error: providersError,
    refetch: refetchProviders,
  } = useOiQuery<TlsDnsProvidersResponse>("/tls/dns-providers/list", {});
  const {
    data: policies,
    loading: policiesLoading,
    error: policiesError,
    refetch: refetchPolicies,
  } = useOiQuery<TlsPoliciesResponse>("/tls/policies/list", {});
  const {
    data: certs,
    loading: certsLoading,
    error: certsError,
    refetch: refetchCerts,
  } = useOiQuery<TlsCertificatesResponse>("/tls/certificates/list", {});
  const {
    data: settings,
    loading: settingsLoading,
    error: settingsError,
    refetch: refetchSettings,
  } = useOiQuery<TlsSettings>("/tls/settings/get", {});

  const { execute, error: actionError, clearError } = useOiAction();
  const writeGuard = useGuard("write");
  const dangerGuard = useGuard("dangerous");

  const [providerDialog, setProviderDialog] = useState(false);
  const [policyDialog, setPolicyDialog] = useState(false);
  const [removingProvider, setRemovingProvider] = useState<string | null>(null);
  const [removingPolicy, setRemovingPolicy] = useState<string | null>(null);

  const refreshAll = () => {
    refetchProviders();
    refetchPolicies();
    refetchCerts();
    refetchSettings();
  };

  const anyLoading =
    providersLoading || policiesLoading || certsLoading || settingsLoading;

  const certById = useMemo(() => {
    const map = new Map<number, TlsCertificate>();
    for (const c of certs?.certificates ?? []) {
      map.set(c.id, c);
    }
    return map;
  }, [certs]);

  return (
    <Box sx={{ p: 3, maxWidth: 1100, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          Certificates
        </Typography>
        <Tooltip title="Refresh">
          <span>
            <IconButton onClick={refreshAll} disabled={anyLoading} size="small">
              <RefreshIcon />
            </IconButton>
          </span>
        </Tooltip>
      </Box>
      <Typography
        variant="body2"
        sx={{ color: "text.secondary", mb: 2 }}
      >
        Manage TLS certificate strategies for ingress hostnames. Hostnames
        without an explicit policy use the default ACME-HTTP-01 issuance
        Caddy provides automatically.
      </Typography>

      {actionError && (
        <Alert severity="error" sx={{ mb: 2 }} onClose={clearError}>
          {actionError.message}
        </Alert>
      )}

      <Stack spacing={4}>
        <SettingsSection
          settings={settings ?? null}
          loading={settingsLoading}
          error={settingsError}
          onSaved={refetchSettings}
          execute={execute}
          writeAllowed={writeGuard.allowed}
          writeReason={writeGuard.reason}
        />

        <PoliciesSection
          policies={policies?.policies ?? []}
          loading={policiesLoading}
          error={policiesError}
          providers={providers?.providers ?? []}
          certById={certById}
          onAdd={() => {
            clearError();
            setPolicyDialog(true);
          }}
          onClear={(hostname) => {
            clearError();
            setRemovingPolicy(hostname);
          }}
          writeAllowed={writeGuard.allowed}
          writeReason={writeGuard.reason}
        />

        <CertificatesSection
          certs={certs?.certificates ?? []}
          loading={certsLoading}
          error={certsError}
        />

        <DnsProvidersSection
          providers={providers?.providers ?? []}
          loading={providersLoading}
          error={providersError}
          onAdd={() => {
            clearError();
            setProviderDialog(true);
          }}
          onDelete={(name) => {
            clearError();
            setRemovingProvider(name);
          }}
          writeAllowed={writeGuard.allowed}
          writeReason={writeGuard.reason}
          dangerAllowed={dangerGuard.allowed}
          dangerReason={dangerGuard.reason}
        />
      </Stack>

      <UpsertProviderDialog
        open={providerDialog}
        onClose={() => setProviderDialog(false)}
        onSubmitted={() => {
          refetchProviders();
          setProviderDialog(false);
        }}
        execute={execute}
      />
      <SetAcmeDnsPolicyDialog
        open={policyDialog}
        providers={providers?.providers ?? []}
        onClose={() => setPolicyDialog(false)}
        onSubmitted={() => {
          refetchPolicies();
          refetchCerts();
          setPolicyDialog(false);
        }}
        execute={execute}
      />
      <ConfirmDialog
        open={removingProvider !== null}
        title="Delete DNS provider"
        body={
          removingProvider
            ? `Delete provider "${removingProvider}"? This is refused if any policy references it.`
            : ""
        }
        confirmLabel="Delete"
        confirmColor="error"
        onClose={() => setRemovingProvider(null)}
        onConfirm={async () => {
          if (!removingProvider) return;
          try {
            await execute("/tls/dns-providers/delete", {
              name: removingProvider,
            });
            refetchProviders();
            setRemovingProvider(null);
          } catch {
            // surfaced via actionError
          }
        }}
      />
      <ConfirmDialog
        open={removingPolicy !== null}
        title="Clear policy"
        body={
          removingPolicy
            ? `Clear the policy for "${removingPolicy}"? The hostname will revert to the default ACME-HTTP-01 strategy.`
            : ""
        }
        confirmLabel="Clear"
        confirmColor="warning"
        onClose={() => setRemovingPolicy(null)}
        onConfirm={async () => {
          if (!removingPolicy) return;
          try {
            await execute("/tls/policies/clear", { hostname: removingPolicy });
            refetchPolicies();
            refetchCerts();
            setRemovingPolicy(null);
          } catch {
            // surfaced via actionError
          }
        }}
      />
    </Box>
  );
}

// ---------------------------------------------------------------------------
// Settings section
// ---------------------------------------------------------------------------

interface SettingsSectionProps {
  settings: TlsSettings | null;
  loading: boolean;
  error: OiQueryError | null;
  onSaved: () => void;
  execute: (path: string, params: unknown) => Promise<unknown>;
  writeAllowed: boolean;
  writeReason: string | null;
}

function SettingsSection({
  settings,
  loading,
  error,
  onSaved,
  execute,
  writeAllowed,
  writeReason,
}: SettingsSectionProps) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);

  const startEdit = () => {
    setDraft(settings?.contact_email ?? "");
    setEditing(true);
  };

  const submit = async () => {
    setSaving(true);
    try {
      await execute("/tls/settings/set", { contact_email: draft.trim() });
      onSaved();
      setEditing(false);
    } finally {
      setSaving(false);
    }
  };

  return (
    <Box>
      <Typography variant="subtitle1" sx={{ fontWeight: 600, mb: 1 }}>
        Settings
      </Typography>
      {error && <OiErrorAlert error={error} />}
      {loading && !settings && <CircularProgress size={20} />}
      <Paper variant="outlined" sx={{ p: 2 }}>
        <Stack
          direction="row"
          spacing={2}
          sx={{ alignItems: "center", flexWrap: "wrap" }}
        >
          <Box sx={{ flexGrow: 1, minWidth: 240 }}>
            <Typography variant="caption" sx={{ color: "text.secondary" }}>
              ACME contact email
            </Typography>
            <Typography sx={{ fontFamily: "monospace" }}>
              {settings?.contact_email
                ? settings.contact_email
                : <em style={{ color: "var(--mui-palette-text-secondary)" }}>not set</em>}
            </Typography>
            <Typography variant="caption" sx={{ color: "text.secondary" }}>
              Used by every ACME account registration. Required before the
              runtime can issue certificates against a public CA.
            </Typography>
          </Box>
          <Tooltip title={writeReason ?? ""}>
            <span>
              <Button
                size="small"
                onClick={startEdit}
                disabled={!writeAllowed || loading}
              >
                Edit
              </Button>
            </span>
          </Tooltip>
        </Stack>
      </Paper>
      <Dialog open={editing} onClose={() => setEditing(false)} fullWidth maxWidth="sm">
        <DialogTitle>Update contact email</DialogTitle>
        <DialogContent>
          <TextField
            autoFocus
            label="Contact email"
            placeholder="ops@example.com"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            fullWidth
            sx={{ mt: 1 }}
            helperText="Leave blank to clear. New value applies on the next renewal pass."
          />
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setEditing(false)} disabled={saving}>
            Cancel
          </Button>
          <Button onClick={submit} variant="contained" disabled={saving}>
            Save
          </Button>
        </DialogActions>
      </Dialog>
    </Box>
  );
}

// ---------------------------------------------------------------------------
// Policies section
// ---------------------------------------------------------------------------

interface PoliciesSectionProps {
  policies: TlsPolicy[];
  loading: boolean;
  error: OiQueryError | null;
  providers: TlsDnsProvider[];
  certById: Map<number, TlsCertificate>;
  onAdd: () => void;
  onClear: (hostname: string) => void;
  writeAllowed: boolean;
  writeReason: string | null;
}

function PoliciesSection({
  policies,
  loading,
  error,
  providers,
  certById,
  onAdd,
  onClear,
  writeAllowed,
  writeReason,
}: PoliciesSectionProps) {
  return (
    <Box>
      <Box sx={{ display: "flex", alignItems: "center", mb: 1, gap: 1 }}>
        <Typography variant="subtitle1" sx={{ fontWeight: 600, flexGrow: 1 }}>
          Per-hostname policies
        </Typography>
        <Tooltip title={writeReason ?? ""}>
          <span>
            <Button
              size="small"
              startIcon={<AddIcon />}
              onClick={onAdd}
              disabled={!writeAllowed || providers.length === 0}
            >
              Bind hostname
            </Button>
          </span>
        </Tooltip>
      </Box>
      <Typography variant="caption" sx={{ color: "text.secondary", mb: 1, display: "block" }}>
        Hostnames absent here use the default Caddy ACME-HTTP-01 strategy.
      </Typography>
      {error && <OiErrorAlert error={error} />}
      {loading && <CircularProgress size={20} />}
      {policies.length === 0 ? (
        <Typography variant="body2" sx={{ color: "text.secondary" }}>
          No operator policies — every TLS-terminating hostname uses the default ACME-HTTP-01.
        </Typography>
      ) : (
        <TableContainer component={Paper} variant="outlined">
          <Table size="small">
            <TableHead>
              <TableRow>
                <TableCell>Hostname</TableCell>
                <TableCell>Strategy</TableCell>
                <TableCell>Source</TableCell>
                <TableCell>Status</TableCell>
                <TableCell>Updated</TableCell>
                <TableCell align="right" />
              </TableRow>
            </TableHead>
            <TableBody>
              {policies.map((p) => {
                const cert =
                  p.strategy === "manual" ? certById.get(p.cert_id) : undefined;
                return (
                  <TableRow key={p.hostname} hover>
                    <TableCell sx={{ fontFamily: "monospace" }}>
                      {p.hostname}
                    </TableCell>
                    <TableCell>
                      <Chip
                        label={p.strategy === "acme_dns" ? "acme-dns" : "manual"}
                        size="small"
                        color={p.strategy === "acme_dns" ? "primary" : "default"}
                        variant="outlined"
                      />
                    </TableCell>
                    <TableCell sx={{ fontFamily: "monospace", fontSize: "0.85rem" }}>
                      {p.strategy === "acme_dns"
                        ? `provider: ${p.dns_provider}`
                        : `cert #${p.cert_id}`}
                    </TableCell>
                    <TableCell>
                      {cert ? (
                        <Stack
                          direction="row"
                          spacing={1}
                          sx={{ alignItems: "center" }}
                        >
                          <Typography variant="caption" sx={{ color: "text.secondary" }}>
                            {formatTime(cert.not_after)}
                          </Typography>
                          {expiryChip(cert.not_after)}
                          {cert.self_signed && (
                            <Chip label="self-signed" size="small" color="warning" variant="outlined" />
                          )}
                        </Stack>
                      ) : (
                        <Typography variant="caption" sx={{ color: "text.secondary" }}>
                          —
                        </Typography>
                      )}
                    </TableCell>
                    <TableCell>
                      <Typography variant="caption" sx={{ color: "text.secondary" }}>
                        {formatTime(p.updated_at)}
                      </Typography>
                    </TableCell>
                    <TableCell align="right">
                      <Tooltip title={writeReason ?? "Clear policy"}>
                        <span>
                          <IconButton
                            size="small"
                            disabled={!writeAllowed}
                            onClick={() => onClear(p.hostname)}
                          >
                            <DeleteOutlineIcon fontSize="small" />
                          </IconButton>
                        </span>
                      </Tooltip>
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        </TableContainer>
      )}
    </Box>
  );
}

// ---------------------------------------------------------------------------
// Certificates section
// ---------------------------------------------------------------------------

interface CertificatesSectionProps {
  certs: TlsCertificate[];
  loading: boolean;
  error: OiQueryError | null;
}

function CertificatesSection({
  certs,
  loading,
  error,
}: CertificatesSectionProps) {
  const grouped = useMemo(() => {
    const m = new Map<string, TlsCertificate[]>();
    for (const c of certs) {
      const arr = m.get(c.hostname) ?? [];
      arr.push(c);
      m.set(c.hostname, arr);
    }
    // active first, then newest superseded
    for (const arr of m.values()) {
      arr.sort((a, b) => {
        if (a.state === "active" && b.state !== "active") return -1;
        if (b.state === "active" && a.state !== "active") return 1;
        return b.created_at - a.created_at;
      });
    }
    return Array.from(m.entries()).sort(([a], [b]) => a.localeCompare(b));
  }, [certs]);

  return (
    <Box>
      <Typography variant="subtitle1" sx={{ fontWeight: 600, mb: 1 }}>
        Stored certificates
      </Typography>
      {error && <OiErrorAlert error={error} />}
      {loading && <CircularProgress size={20} />}
      {certs.length === 0 ? (
        <Typography variant="body2" sx={{ color: "text.secondary" }}>
          No stored certificates yet. ACME-DNS issuance, manual upload, or the
          CSR flow will populate this list.
        </Typography>
      ) : (
        <Stack spacing={1}>
          {grouped.map(([hostname, rows]) => (
            <Paper key={hostname} variant="outlined" sx={{ p: 2 }}>
              <Typography sx={{ fontFamily: "monospace", fontWeight: 500, mb: 1 }}>
                {hostname}
              </Typography>
              <Table size="small">
                <TableHead>
                  <TableRow>
                    <TableCell>State</TableCell>
                    <TableCell>Origin</TableCell>
                    <TableCell>Issuer</TableCell>
                    <TableCell>Not after</TableCell>
                    <TableCell>Serial</TableCell>
                  </TableRow>
                </TableHead>
                <TableBody>
                  {rows.map((c) => (
                    <TableRow key={c.id}>
                      <TableCell>
                        <Stack
                          direction="row"
                          spacing={1}
                          sx={{ alignItems: "center" }}
                        >
                          <Chip
                            label={c.state.replace("_", " ")}
                            size="small"
                            color={
                              c.state === "active"
                                ? "success"
                                : c.state === "failed"
                                  ? "error"
                                  : "default"
                            }
                            variant={c.state === "active" ? "filled" : "outlined"}
                          />
                          {expiryChip(c.not_after)}
                          {c.self_signed && (
                            <Chip
                              label="self-signed"
                              size="small"
                              color="warning"
                              variant="outlined"
                            />
                          )}
                        </Stack>
                      </TableCell>
                      <TableCell>
                        <Chip label={c.origin} size="small" variant="outlined" />
                      </TableCell>
                      <TableCell sx={{ fontFamily: "monospace", fontSize: "0.8rem" }}>
                        {c.issuer ?? "—"}
                      </TableCell>
                      <TableCell sx={{ fontSize: "0.85rem" }}>
                        {formatTime(c.not_after)}
                      </TableCell>
                      <TableCell sx={{ fontFamily: "monospace", fontSize: "0.8rem" }}>
                        {c.serial ?? "—"}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </Paper>
          ))}
        </Stack>
      )}
    </Box>
  );
}

// ---------------------------------------------------------------------------
// DNS providers section
// ---------------------------------------------------------------------------

interface DnsProvidersSectionProps {
  providers: TlsDnsProvider[];
  loading: boolean;
  error: OiQueryError | null;
  onAdd: () => void;
  onDelete: (name: string) => void;
  writeAllowed: boolean;
  writeReason: string | null;
  dangerAllowed: boolean;
  dangerReason: string | null;
}

function DnsProvidersSection({
  providers,
  loading,
  error,
  onAdd,
  onDelete,
  writeAllowed,
  writeReason,
  dangerAllowed,
  dangerReason,
}: DnsProvidersSectionProps) {
  return (
    <Box>
      <Box sx={{ display: "flex", alignItems: "center", mb: 1, gap: 1 }}>
        <Typography variant="subtitle1" sx={{ fontWeight: 600, flexGrow: 1 }}>
          DNS providers
        </Typography>
        <Tooltip title={writeReason ?? ""}>
          <span>
            <Button
              size="small"
              startIcon={<AddIcon />}
              onClick={onAdd}
              disabled={!writeAllowed}
            >
              Add
            </Button>
          </span>
        </Tooltip>
      </Box>
      <Typography variant="caption" sx={{ color: "text.secondary", mb: 1, display: "block" }}>
        Credentials used by the ACME-DNS-01 strategy. Stored encrypted at
        rest; never returned by any operator endpoint.
      </Typography>
      {error && <OiErrorAlert error={error} />}
      {loading && <CircularProgress size={20} />}
      {providers.length === 0 ? (
        <Typography variant="body2" sx={{ color: "text.secondary" }}>
          No DNS providers configured. Add one to enable ACME-DNS-01.
        </Typography>
      ) : (
        <TableContainer component={Paper} variant="outlined">
          <Table size="small">
            <TableHead>
              <TableRow>
                <TableCell>Name</TableCell>
                <TableCell>Kind</TableCell>
                <TableCell>Updated</TableCell>
                <TableCell align="right" />
              </TableRow>
            </TableHead>
            <TableBody>
              {providers.map((p) => (
                <TableRow key={p.name} hover>
                  <TableCell sx={{ fontFamily: "monospace" }}>{p.name}</TableCell>
                  <TableCell>
                    <Chip label={p.kind} size="small" variant="outlined" />
                  </TableCell>
                  <TableCell>
                    <Typography variant="caption" sx={{ color: "text.secondary" }}>
                      {formatTime(p.updated_at)}
                    </Typography>
                  </TableCell>
                  <TableCell align="right">
                    <Tooltip title={dangerReason ?? "Delete provider"}>
                      <span>
                        <IconButton
                          size="small"
                          disabled={!dangerAllowed}
                          onClick={() => onDelete(p.name)}
                        >
                          <DeleteOutlineIcon fontSize="small" />
                        </IconButton>
                      </span>
                    </Tooltip>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </TableContainer>
      )}
    </Box>
  );
}

// ---------------------------------------------------------------------------
// Dialogs
// ---------------------------------------------------------------------------

interface UpsertProviderDialogProps {
  open: boolean;
  onClose: () => void;
  onSubmitted: () => void;
  execute: (path: string, params: unknown) => Promise<unknown>;
}

function UpsertProviderDialog({
  open,
  onClose,
  onSubmitted,
  execute,
}: UpsertProviderDialogProps) {
  const [name, setName] = useState("");
  const [accessKeyId, setAccessKeyId] = useState("");
  const [secretAccessKey, setSecretAccessKey] = useState("");
  const [region, setRegion] = useState("us-east-1");
  const [submitting, setSubmitting] = useState(false);

  const reset = () => {
    setName("");
    setAccessKeyId("");
    setSecretAccessKey("");
    setRegion("us-east-1");
  };

  const close = () => {
    reset();
    onClose();
  };

  const trimmedName = name.trim();
  const valid =
    trimmedName.length > 0 && accessKeyId.length > 0 && secretAccessKey.length > 0;

  const submit = async () => {
    setSubmitting(true);
    try {
      await execute("/tls/dns-providers/upsert", {
        name: trimmedName,
        kind: "route53",
        config: {
          access_key_id: accessKeyId,
          secret_access_key: secretAccessKey,
          region,
        },
      });
      reset();
      onSubmitted();
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onClose={close} fullWidth maxWidth="sm">
      <DialogTitle>Add DNS provider</DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 1 }}>
          <TextField
            label="Name"
            placeholder="e.g. primary, ops-account"
            value={name}
            onChange={(e) => setName(e.target.value)}
            fullWidth
            helperText="Operator-chosen identifier referenced by policies"
          />
          <TextField label="Kind" value="Route 53" disabled fullWidth />
          <TextField
            label="Access key ID"
            value={accessKeyId}
            onChange={(e) => setAccessKeyId(e.target.value)}
            fullWidth
            slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
          />
          <TextField
            label="Secret access key"
            type="password"
            value={secretAccessKey}
            onChange={(e) => setSecretAccessKey(e.target.value)}
            fullWidth
            slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
          />
          <TextField
            label="Region"
            value={region}
            onChange={(e) => setRegion(e.target.value)}
            fullWidth
            helperText="Route 53 itself is global; this is the SDK signer region"
          />
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={close} disabled={submitting}>
          Cancel
        </Button>
        <Button
          onClick={submit}
          variant="contained"
          disabled={!valid || submitting}
        >
          Save
        </Button>
      </DialogActions>
    </Dialog>
  );
}

interface SetAcmeDnsPolicyDialogProps {
  open: boolean;
  providers: TlsDnsProvider[];
  onClose: () => void;
  onSubmitted: () => void;
  execute: (path: string, params: unknown) => Promise<unknown>;
}

function SetAcmeDnsPolicyDialog({
  open,
  providers,
  onClose,
  onSubmitted,
  execute,
}: SetAcmeDnsPolicyDialogProps) {
  const [hostname, setHostname] = useState("");
  const [provider, setProvider] = useState(providers[0]?.name ?? "");
  const [submitting, setSubmitting] = useState(false);

  // Keep provider selection in sync with the available list when it loads.
  if (provider === "" && providers.length > 0) {
    setProvider(providers[0].name);
  }

  const reset = () => {
    setHostname("");
  };

  const close = () => {
    reset();
    onClose();
  };

  const trimmedHost = hostname.trim();
  const isExact = trimmedHost.length > 0 && !trimmedHost.includes("*");
  const valid = trimmedHost.length > 0 && provider.length > 0;

  const submit = async () => {
    setSubmitting(true);
    try {
      await execute("/tls/policies/set-acme-dns", {
        hostname: trimmedHost,
        dns_provider: provider,
      });
      reset();
      onSubmitted();
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onClose={close} fullWidth maxWidth="sm">
      <DialogTitle>Bind hostname to ACME-DNS</DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 1 }}>
          <TextField
            label="Hostname or wildcard"
            placeholder="e.g. example.com, *.example.com, *"
            value={hostname}
            onChange={(e) => setHostname(e.target.value)}
            fullWidth
            slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
            helperText={
              <span>
                Exact (<code>example.com</code>), single-asterisk wildcard
                (<code>*.example.com</code>), or catch-all (<code>*</code>).
                Most-specific match wins.
              </span>
            }
          />
          <TextField
            select
            label="DNS provider"
            value={provider}
            onChange={(e) => setProvider(e.target.value)}
            fullWidth
          >
            {providers.map((p) => (
              <MenuItem key={p.name} value={p.name}>
                {p.name} ({p.kind})
              </MenuItem>
            ))}
          </TextField>
          {isExact && (
            <Typography variant="caption" sx={{ color: "text.secondary" }}>
              When the global contact email is configured, the daemon will
              auto-fire a one-shot ACME-DNS issuance for this exact hostname
              if no active cert exists yet.
            </Typography>
          )}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={close} disabled={submitting}>
          Cancel
        </Button>
        <Button
          onClick={submit}
          variant="contained"
          disabled={!valid || submitting}
          startIcon={isExact ? <BoltIcon /> : undefined}
        >
          Save
        </Button>
      </DialogActions>
    </Dialog>
  );
}

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  body: string;
  confirmLabel: string;
  confirmColor: "error" | "warning" | "primary";
  onClose: () => void;
  onConfirm: () => void;
}

function ConfirmDialog({
  open,
  title,
  body,
  confirmLabel,
  confirmColor,
  onClose,
  onConfirm,
}: ConfirmDialogProps) {
  return (
    <Dialog open={open} onClose={onClose} fullWidth maxWidth="sm">
      <DialogTitle>{title}</DialogTitle>
      <DialogContent>
        <Typography>{body}</Typography>
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose}>Cancel</Button>
        <Button onClick={onConfirm} variant="contained" color={confirmColor}>
          {confirmLabel}
        </Button>
      </DialogActions>
    </Dialog>
  );
}
