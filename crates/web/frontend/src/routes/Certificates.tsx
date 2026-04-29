import AddIcon from "@mui/icons-material/Add";
import BoltIcon from "@mui/icons-material/Bolt";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutlineOutlined";
import RefreshIcon from "@mui/icons-material/Refresh";
import {
  Alert,
  Box,
  Button,
  Checkbox,
  Chip,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  FormControlLabel,
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
  Typography,
} from "@mui/material";
import { useState } from "react";
import {
  IconActionButton,
  OutlinedActionButton,
  SolidActionButton,
} from "../components/ActionButton";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { TlsHostnamesTable } from "../components/TlsHostnamesTable";
import type { OiQueryError } from "../hooks/useOi";
import { useOiQuery } from "../hooks/useOi";
import { useOiAction } from "../hooks/useOiAction";
import type {
  TlsCertificate,
  TlsCertificatesResponse,
  TlsCsrBeginResponse,
  TlsCsrGetResponse,
  TlsDnsProvider,
  TlsDnsProvidersResponse,
  TlsPoliciesResponse,
  TlsPolicy,
  TlsSettings,
} from "../lib/types";

function formatTime(unix: number | null): string {
  if (!unix) return "—";
  return new Date(unix * 1000).toLocaleString();
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
    data: settings,
    loading: settingsLoading,
    error: settingsError,
    refetch: refetchSettings,
  } = useOiQuery<TlsSettings>("/tls/settings/get", {});
  const {
    data: certs,
    loading: certsLoading,
    error: certsError,
    refetch: refetchCerts,
  } = useOiQuery<TlsCertificatesResponse>("/tls/certificates/list", {});

  const { execute, error: actionError, clearError } = useOiAction();

  const [providerDialog, setProviderDialog] = useState(false);
  const [policyDialog, setPolicyDialog] = useState(false);
  const [uploadDialog, setUploadDialog] = useState(false);
  const [csrBeginDialog, setCsrBeginDialog] = useState(false);
  const [csrShow, setCsrShow] = useState<{ id: number; csrPem: string } | null>(null);
  const [csrUpload, setCsrUpload] = useState<TlsCertificate | null>(null);
  const [removingProvider, setRemovingProvider] = useState<string | null>(null);
  const [removingPolicy, setRemovingPolicy] = useState<string | null>(null);
  const [deletingCert, setDeletingCert] = useState<TlsCertificate | null>(null);

  const refreshAll = () => {
    refetchProviders();
    refetchPolicies();
    refetchSettings();
    refetchCerts();
  };

  const anyLoading =
    providersLoading || policiesLoading || settingsLoading || certsLoading;

  return (
    <Box sx={{ p: 3, maxWidth: 1100, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          TLS Certificates
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
      <Typography
        variant="body2"
        sx={{ color: "text.secondary", mb: 2 }}
      >
        Per-domain rollup of every TLS-terminating ingress in the system.
        Domains without an explicit policy use the default (TLS/HTTP/internal)
        issuance Caddy provides automatically.
      </Typography>

      {actionError && (
        <Alert severity="error" sx={{ mb: 2 }} onClose={clearError}>
          {actionError.message}
        </Alert>
      )}

      <Stack spacing={4}>
        <TlsHostnamesTable />

        <SettingsSection
          settings={settings ?? null}
          loading={settingsLoading}
          error={settingsError}
          onSaved={refetchSettings}
        />

        <PoliciesSection
          policies={policies?.policies ?? []}
          loading={policiesLoading}
          error={policiesError}
          providers={providers?.providers ?? []}
          onAdd={() => {
            clearError();
            setPolicyDialog(true);
          }}
          onClear={(hostname) => {
            clearError();
            setRemovingPolicy(hostname);
          }}
        />

        <ManualCertsSection
          certs={(certs?.certificates ?? []).filter((c) => c.origin !== "acme_dns")}
          loading={certsLoading}
          error={certsError}
          onUpload={() => {
            clearError();
            setUploadDialog(true);
          }}
          onDelete={(c) => {
            clearError();
            setDeletingCert(c);
          }}
          onGenerateCsr={() => {
            clearError();
            setCsrBeginDialog(true);
          }}
          onShowCsr={async (c) => {
            clearError();
            try {
              const result = (await execute("/tls/certificates/csr/get", {
                id: c.id,
              })) as TlsCsrGetResponse;
              setCsrShow({ id: result.id, csrPem: result.csr_pem });
            } catch {
              // surfaced via actionError
            }
          }}
          onUploadCsrCert={(c) => {
            clearError();
            setCsrUpload(c);
          }}
          onCancelCsr={async (c) => {
            clearError();
            try {
              await execute("/tls/certificates/csr/cancel", { id: c.id });
              refetchCerts();
            } catch {
              // surfaced via actionError
            }
          }}
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
        />
      </Stack>

      <UpsertProviderDialog
        open={providerDialog}
        onClose={() => setProviderDialog(false)}
        onSubmitted={() => {
          refetchProviders();
          // The first provider upsert can auto-create a `*` policy, so
          // refresh the policies + certs lists too rather than requiring
          // an operator reload.
          refetchPolicies();
          setProviderDialog(false);
        }}
      />
      <SetAcmeDnsPolicyDialog
        open={policyDialog}
        providers={providers?.providers ?? []}
        onClose={() => setPolicyDialog(false)}
        onSubmitted={() => {
          refetchPolicies();
          setPolicyDialog(false);
        }}
      />
      <UploadManualCertDialog
        open={uploadDialog}
        onClose={() => setUploadDialog(false)}
        onSubmitted={() => {
          refetchCerts();
          setUploadDialog(false);
        }}
      />
      <CsrBeginDialog
        open={csrBeginDialog}
        onClose={() => setCsrBeginDialog(false)}
        onSubmitted={(result) => {
          refetchCerts();
          setCsrBeginDialog(false);
          setCsrShow({ id: result.id, csrPem: result.csr_pem });
        }}
      />
      <CsrShowDialog
        open={csrShow !== null}
        csrId={csrShow?.id ?? null}
        csrPem={csrShow?.csrPem ?? ""}
        onClose={() => setCsrShow(null)}
      />
      <CsrUploadCertDialog
        open={csrUpload !== null}
        cert={csrUpload}
        onClose={() => setCsrUpload(null)}
        onSubmitted={() => {
          refetchCerts();
          setCsrUpload(null);
        }}
      />
      <ConfirmDialog
        open={deletingCert !== null}
        title="Delete certificate"
        body={
          deletingCert
            ? `Delete cert #${deletingCert.id} (${deletingCert.hostname})? Refused if a manual policy still references it.`
            : ""
        }
        confirmLabel="Delete"
        confirmColor="error"
        safety="dangerous"
        onClose={() => setDeletingCert(null)}
        onConfirm={async () => {
          if (!deletingCert) return;
          try {
            await execute("/tls/certificates/delete", { id: deletingCert.id });
            refetchCerts();
            setDeletingCert(null);
          } catch {
            // surfaced via actionError
          }
        }}
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
        safety="dangerous"
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
            ? `Clear the policy for "${removingPolicy}"? The domain will revert to the default Caddy issuance strategy.`
            : ""
        }
        confirmLabel="Clear"
        confirmColor="warning"
        safety="write"
        onClose={() => setRemovingPolicy(null)}
        onConfirm={async () => {
          if (!removingPolicy) return;
          try {
            await execute("/tls/policies/clear", { hostname: removingPolicy });
            refetchPolicies();
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
}

function SettingsSection({
  settings,
  loading,
  error,
  onSaved,
}: SettingsSectionProps) {
  const [editing, setEditing] = useState(false);
  const [emailDraft, setEmailDraft] = useState("");
  const [profileDraft, setProfileDraft] = useState("");
  const [shortLived, setShortLived] = useState(false);
  const [saving, setSaving] = useState(false);
  const { execute, error: submitError, clearError } = useOiAction();

  const startEdit = () => {
    setEmailDraft(settings?.contact_email ?? "");
    const stored = settings?.cert_profile ?? "";
    setShortLived(stored === "shortlived");
    setProfileDraft(stored === "shortlived" ? "" : stored);
    clearError();
    setEditing(true);
  };

  const closeEdit = () => {
    clearError();
    setEditing(false);
  };

  const submit = async () => {
    setSaving(true);
    try {
      // Send both fields explicitly: contact_email is always present
      // (empty string clears), cert_profile is null when neither short-
      // lived is selected nor a custom profile is typed.
      const trimmedProfile = profileDraft.trim();
      const profile: string | null = shortLived
        ? "shortlived"
        : trimmedProfile.length > 0
          ? trimmedProfile
          : null;
      await execute("/tls/settings/set", {
        contact_email: emailDraft.trim(),
        cert_profile: profile,
      });
      onSaved();
      setEditing(false);
    } catch {
      // surfaced inline via `submitError`
    } finally {
      setSaving(false);
    }
  };

  const profileSummary = (() => {
    const p = settings?.cert_profile;
    if (!p) return "default (CA picks the profile, ~90 days at Let's Encrypt)";
    if (p === "shortlived") return "shortlived (~6-day certificates at Let's Encrypt)";
    return p;
  })();

  return (
    <Box>
      <Typography variant="subtitle1" sx={{ fontWeight: 600, mb: 1 }}>
        Settings
      </Typography>
      {error && <OiErrorAlert error={error} />}
      {!settings ? (
        // Don't render the Paper with defaulted-out values while
        // loading — an operator who lands here mid-fetch could mistake
        // an empty contact email and "default" cert profile for the
        // actually-stored state and start reconfiguring on top of it.
        <Box sx={{ display: "flex", alignItems: "center", gap: 1, color: "text.secondary" }}>
          <CircularProgress size={16} />
          <Typography variant="body2">Loading TLS settings…</Typography>
        </Box>
      ) : (
        <Paper variant="outlined" sx={{ p: 2 }}>
          <Stack
            direction="row"
            spacing={2}
            sx={{ alignItems: "flex-start", flexWrap: "wrap" }}
          >
            <Box sx={{ flexGrow: 1, minWidth: 240 }}>
              <Typography variant="caption" sx={{ color: "text.secondary" }}>
                ACME contact email
              </Typography>
              <Typography sx={{ fontFamily: "monospace" }}>
                {settings.contact_email
                  ? settings.contact_email
                  : <em style={{ color: "var(--mui-palette-text-secondary)" }}>not set</em>}
              </Typography>
              <Typography variant="caption" sx={{ color: "text.secondary", mb: 1, display: "block" }}>
                Used by every ACME account registration. Required before the
                runtime can issue certificates against a public CA.
              </Typography>
              <Typography variant="caption" sx={{ color: "text.secondary" }}>
                Cert profile
              </Typography>
              <Typography sx={{ fontFamily: "monospace" }}>
                {profileSummary}
              </Typography>
            </Box>
            <OutlinedActionButton
              safety="write"
              size="small"
              onClick={startEdit}
              disabled={loading}
            >
              Edit
            </OutlinedActionButton>
          </Stack>
        </Paper>
      )}
      <Dialog open={editing} onClose={closeEdit} fullWidth maxWidth="sm">
        <DialogTitle>TLS settings</DialogTitle>
        <DialogContent>
          <Stack spacing={2} sx={{ mt: 1 }}>
            {submitError && (
              <Alert severity="error" onClose={clearError}>
                {submitError.message}
              </Alert>
            )}
            <TextField
              autoFocus
              label="Contact email"
              placeholder="ops@example.com"
              value={emailDraft}
              onChange={(e) => setEmailDraft(e.target.value)}
              fullWidth
              helperText="Leave blank to clear. Required before public-CA issuance."
            />
            <FormControlLabel
              control={
                <Checkbox
                  checked={shortLived}
                  onChange={(e) => {
                    setShortLived(e.target.checked);
                    if (e.target.checked) setProfileDraft("");
                  }}
                />
              }
              label="Use Let's Encrypt short-lived (~6-day) certificates"
            />
            <TextField
              label="Custom ACME profile (advanced)"
              placeholder="leave blank for the CA's default"
              value={profileDraft}
              onChange={(e) => setProfileDraft(e.target.value)}
              disabled={shortLived}
              fullWidth
              slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
              helperText={
                shortLived
                  ? "Disabled because the short-lived option above is selected."
                  : "Forwarded on every ACME order. Most operators want either the short-lived toggle or no profile at all."
              }
            />
          </Stack>
        </DialogContent>
        <DialogActions>
          <Button onClick={closeEdit} disabled={saving}>
            Cancel
          </Button>
          <SolidActionButton safety="write" onClick={submit} disabled={saving}>
            Save
          </SolidActionButton>
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
  onAdd: () => void;
  onClear: (hostname: string) => void;
}

function PoliciesSection({
  policies,
  loading,
  error,
  providers,
  onAdd,
  onClear,
}: PoliciesSectionProps) {
  return (
    <Box>
      <Box sx={{ display: "flex", alignItems: "center", mb: 1, gap: 1 }}>
        <Typography variant="subtitle1" sx={{ fontWeight: 600, flexGrow: 1 }}>
          Per-domain policies
        </Typography>
        <OutlinedActionButton
          safety="write"
          size="small"
          startIcon={<AddIcon />}
          onClick={onAdd}
          disabled={providers.length === 0}
        >
          Bind domain
        </OutlinedActionButton>
      </Box>
      <Typography variant="caption" sx={{ color: "text.secondary", mb: 1, display: "block" }}>
        Domains absent here use the default Caddy issuance strategy (TLS/HTTP/internal).
      </Typography>
      {error && <OiErrorAlert error={error} />}
      {loading && <CircularProgress size={20} />}
      {policies.length === 0 ? (
        <Typography variant="body2" sx={{ color: "text.secondary" }}>
          No operator policies — every TLS-terminating domain uses the Caddy default.
        </Typography>
      ) : (
        <TableContainer component={Paper} variant="outlined">
          <Table size="small">
            <TableHead>
              <TableRow>
                <TableCell>Domain pattern</TableCell>
                <TableCell>Strategy</TableCell>
                <TableCell>Source</TableCell>
                <TableCell>Updated</TableCell>
                <TableCell align="right" />
              </TableRow>
            </TableHead>
            <TableBody>
              {policies.map((p) => (
                <TableRow key={p.hostname} hover>
                  <TableCell sx={{ fontFamily: "monospace" }}>
                    {p.hostname}
                  </TableCell>
                  <TableCell>
                    <Chip
                      label="acme-dns"
                      size="small"
                      color="primary"
                      variant="outlined"
                    />
                  </TableCell>
                  <TableCell sx={{ fontFamily: "monospace", fontSize: "0.85rem" }}>
                    provider: {p.dns_provider}
                  </TableCell>
                  <TableCell>
                    <Typography variant="caption" sx={{ color: "text.secondary" }}>
                      {formatTime(p.updated_at)}
                    </Typography>
                  </TableCell>
                  <TableCell align="right">
                    <IconActionButton
                      safety="write"
                      tooltip="Clear policy"
                      onClick={() => onClear(p.hostname)}
                    >
                      <DeleteOutlineIcon fontSize="small" />
                    </IconActionButton>
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
// Manual / CSR-derived certificates section
// ---------------------------------------------------------------------------

interface ManualCertsSectionProps {
  certs: TlsCertificate[];
  loading: boolean;
  error: OiQueryError | null;
  onUpload: () => void;
  onDelete: (cert: TlsCertificate) => void;
  onGenerateCsr: () => void;
  onShowCsr: (cert: TlsCertificate) => void;
  onUploadCsrCert: (cert: TlsCertificate) => void;
  onCancelCsr: (cert: TlsCertificate) => void;
}

function ManualCertsSection({
  certs,
  loading,
  error,
  onUpload,
  onDelete,
  onGenerateCsr,
  onShowCsr,
  onUploadCsrCert,
  onCancelCsr,
}: ManualCertsSectionProps) {
  return (
    <Box>
      <Box sx={{ display: "flex", alignItems: "center", mb: 1, gap: 1 }}>
        <Typography variant="subtitle1" sx={{ fontWeight: 600, flexGrow: 1 }}>
          Stored certificates
        </Typography>
        <OutlinedActionButton
          safety="write"
          size="small"
          onClick={onGenerateCsr}
        >
          Generate CSR
        </OutlinedActionButton>
        <OutlinedActionButton
          safety="write"
          size="small"
          startIcon={<AddIcon />}
          onClick={onUpload}
        >
          Upload manual cert
        </OutlinedActionButton>
      </Box>
      <Typography variant="caption" sx={{ color: "text.secondary", mb: 1, display: "block" }}>
        Manually-uploaded and CSR-derived certificates. The runtime auto-binds each
        certificate to every domain its SAN list covers, so no per-domain
        configuration step is needed.
      </Typography>
      {error && <OiErrorAlert error={error} />}
      {loading && certs.length === 0 && <CircularProgress size={20} />}
      {certs.length === 0 ? (
        <Typography variant="body2" sx={{ color: "text.secondary" }}>
          No manual or CSR-derived certificates stored.
        </Typography>
      ) : (
        <TableContainer component={Paper} variant="outlined">
          <Table size="small">
            <TableHead>
              <TableRow>
                <TableCell>#</TableCell>
                <TableCell>Primary SAN</TableCell>
                <TableCell>Origin</TableCell>
                <TableCell>State</TableCell>
                <TableCell>Issuer</TableCell>
                <TableCell>Not after</TableCell>
                <TableCell align="right" />
              </TableRow>
            </TableHead>
            <TableBody>
              {certs.map((c) => {
                const pending = c.state === "csr_pending";
                return (
                  <TableRow key={c.id} hover>
                    <TableCell sx={{ fontFamily: "monospace" }}>{c.id}</TableCell>
                    <TableCell sx={{ fontFamily: "monospace" }}>{c.hostname}</TableCell>
                    <TableCell>
                      <Chip label={c.origin} size="small" variant="outlined" />
                    </TableCell>
                    <TableCell>
                      <Stack direction="row" spacing={0.5} sx={{ alignItems: "center" }}>
                        <Chip
                          label={c.state}
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
                        {c.self_signed && (
                          <Chip label="self-signed" size="small" color="warning" variant="outlined" />
                        )}
                      </Stack>
                    </TableCell>
                    <TableCell sx={{ fontFamily: "monospace", fontSize: "0.8rem" }}>
                      {c.issuer ?? "—"}
                    </TableCell>
                    <TableCell sx={{ fontSize: "0.85rem" }}>{formatTime(c.not_after)}</TableCell>
                    <TableCell align="right">
                      {pending ? (
                        <>
                          <OutlinedActionButton
                            safety="read"
                            tooltip="Show CSR PEM again"
                            size="small"
                            onClick={() => onShowCsr(c)}
                          >
                            Show CSR
                          </OutlinedActionButton>
                          <OutlinedActionButton
                            safety="write"
                            tooltip="Upload signed cert"
                            size="small"
                            onClick={() => onUploadCsrCert(c)}
                          >
                            Upload cert
                          </OutlinedActionButton>
                          <IconActionButton
                            safety="write"
                            tooltip="Cancel CSR (deletes the keypair)"
                            onClick={() => onCancelCsr(c)}
                          >
                            <DeleteOutlineIcon fontSize="small" />
                          </IconActionButton>
                        </>
                      ) : (
                        <IconActionButton
                          safety="dangerous"
                          tooltip="Delete"
                          onClick={() => onDelete(c)}
                        >
                          <DeleteOutlineIcon fontSize="small" />
                        </IconActionButton>
                      )}
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
// DNS providers section
// ---------------------------------------------------------------------------

interface DnsProvidersSectionProps {
  providers: TlsDnsProvider[];
  loading: boolean;
  error: OiQueryError | null;
  onAdd: () => void;
  onDelete: (name: string) => void;
}

function DnsProvidersSection({
  providers,
  loading,
  error,
  onAdd,
  onDelete,
}: DnsProvidersSectionProps) {
  return (
    <Box>
      <Box sx={{ display: "flex", alignItems: "center", mb: 1, gap: 1 }}>
        <Typography variant="subtitle1" sx={{ fontWeight: 600, flexGrow: 1 }}>
          DNS providers
        </Typography>
        <OutlinedActionButton
          safety="write"
          size="small"
          startIcon={<AddIcon />}
          onClick={onAdd}
        >
          Add
        </OutlinedActionButton>
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
                    <IconActionButton
                      safety="dangerous"
                      tooltip="Delete provider"
                      onClick={() => onDelete(p.name)}
                    >
                      <DeleteOutlineIcon fontSize="small" />
                    </IconActionButton>
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
}

function UpsertProviderDialog({
  open,
  onClose,
  onSubmitted,
}: UpsertProviderDialogProps) {
  const [name, setName] = useState("");
  const [accessKeyId, setAccessKeyId] = useState("");
  const [secretAccessKey, setSecretAccessKey] = useState("");
  const [region, setRegion] = useState("us-east-1");
  const [submitting, setSubmitting] = useState(false);
  const { execute, error, clearError } = useOiAction();

  const reset = () => {
    setName("");
    setAccessKeyId("");
    setSecretAccessKey("");
    setRegion("us-east-1");
    clearError();
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
    } catch {
      // surfaced inline via `error`
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onClose={close} fullWidth maxWidth="sm">
      <DialogTitle>Add DNS provider</DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 1 }}>
          {error && (
            <Alert severity="error" onClose={clearError}>
              {error.message}
            </Alert>
          )}
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
        <SolidActionButton
          safety="write"
          onClick={submit}
          disabled={!valid || submitting}
        >
          Save
        </SolidActionButton>
      </DialogActions>
    </Dialog>
  );
}

interface SetAcmeDnsPolicyDialogProps {
  open: boolean;
  providers: TlsDnsProvider[];
  onClose: () => void;
  onSubmitted: () => void;
}

function SetAcmeDnsPolicyDialog({
  open,
  providers,
  onClose,
  onSubmitted,
}: SetAcmeDnsPolicyDialogProps) {
  const [hostname, setHostname] = useState("");
  const [provider, setProvider] = useState(providers[0]?.name ?? "");
  const [submitting, setSubmitting] = useState(false);
  const { execute, error, clearError } = useOiAction();

  // Keep provider selection in sync with the available list when it loads.
  if (provider === "" && providers.length > 0) {
    setProvider(providers[0].name);
  }

  const reset = () => {
    setHostname("");
    clearError();
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
    } catch {
      // surfaced inline via `error`
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onClose={close} fullWidth maxWidth="sm">
      <DialogTitle>Bind domain to ACME-DNS</DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 1 }}>
          {error && (
            <Alert severity="error" onClose={clearError}>
              {error.message}
            </Alert>
          )}
          <TextField
            label="Domain or wildcard"
            placeholder="e.g. example.com, *.example.com, *"
            value={hostname}
            onChange={(e) => setHostname(e.target.value)}
            fullWidth
            slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
            helperText={
              <span>
                Exact (<code>example.com</code>), shell-glob subdomain
                wildcard (<code>*.example.com</code> covers
                <code>foo.example.com</code> and{" "}
                <code>a.b.example.com</code> — any depth), or catch-all
                (<code>*</code>). Most-specific match wins, so a more
                specific pattern overrides a broader one.
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
              auto-fire a one-shot ACME-DNS issuance for this exact domain
              if no active cert exists yet.
            </Typography>
          )}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={close} disabled={submitting}>
          Cancel
        </Button>
        <SolidActionButton
          safety="write"
          onClick={submit}
          disabled={!valid || submitting}
          startIcon={isExact ? <BoltIcon /> : undefined}
        >
          Save
        </SolidActionButton>
      </DialogActions>
    </Dialog>
  );
}

interface UploadManualCertDialogProps {
  open: boolean;
  onClose: () => void;
  onSubmitted: () => void;
}

function UploadManualCertDialog({
  open,
  onClose,
  onSubmitted,
}: UploadManualCertDialogProps) {
  const [certPem, setCertPem] = useState("");
  const [keyPem, setKeyPem] = useState("");
  const [note, setNote] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [result, setResult] = useState<
    { primary_san?: string; san_dns_names?: string[]; warnings: string[] } | null
  >(null);
  // Owned action state — keeps errors visible inside the dialog
  // rather than letting them propagate to the page-level toast that
  // sits behind the open modal.
  const { execute, error, clearError } = useOiAction();

  const reset = () => {
    setCertPem("");
    setKeyPem("");
    setNote("");
    setResult(null);
    clearError();
  };

  const close = () => {
    reset();
    onClose();
  };

  const valid = certPem.trim().length > 0 && keyPem.trim().length > 0;

  const submit = async () => {
    setSubmitting(true);
    try {
      const params: Record<string, unknown> = {
        cert_pem: certPem,
        key_pem: keyPem,
      };
      const trimmedNote = note.trim();
      if (trimmedNote.length > 0) params.note = trimmedNote;
      const r = (await execute("/tls/certificates/upload-manual", params)) as {
        primary_san?: string;
        san_dns_names?: string[];
        warnings?: string[];
      };
      const warns = r.warnings ?? [];
      if (warns.length === 0) {
        reset();
        onSubmitted();
      } else {
        setResult({
          primary_san: r.primary_san,
          san_dns_names: r.san_dns_names,
          warnings: warns,
        });
      }
    } catch {
      // surfaced inline via `error` from useOiAction
    } finally {
      setSubmitting(false);
    }
  };

  const acceptAndClose = () => {
    reset();
    onSubmitted();
  };

  return (
    <Dialog open={open} onClose={close} fullWidth maxWidth="md">
      <DialogTitle>Upload manual certificate</DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 1 }}>
          {error && (
            <Alert severity="error" onClose={clearError}>
              {error.message}
            </Alert>
          )}
          <PemField
            label="Certificate PEM"
            placeholder="-----BEGIN CERTIFICATE-----..."
            accept=".pem,.crt,.cer"
            minRows={6}
            value={certPem}
            onChange={setCertPem}
            helperText="Paste, drop, or open a PEM file. Leaf cert plus optional intermediates. The runtime will auto-bind the cert to every hostname its SANs cover."
          />
          <PemField
            label="Private key PEM"
            placeholder="-----BEGIN PRIVATE KEY-----..."
            accept=".pem,.key"
            minRows={5}
            value={keyPem}
            onChange={setKeyPem}
            helperText="PKCS#8 PEM. Stored encrypted at rest; never returned by any operator endpoint."
          />
          <TextField
            label="Note (optional)"
            value={note}
            onChange={(e) => setNote(e.target.value)}
            fullWidth
          />
          {result && (
            <Alert severity="warning">
              Uploaded with warnings: {result.warnings.join(", ")}.
              {result.san_dns_names && result.san_dns_names.length > 0 && (
                <>
                  {" "}Will cover: {result.san_dns_names.join(", ")}.
                </>
              )}
            </Alert>
          )}
        </Stack>
      </DialogContent>
      <DialogActions>
        {result ? (
          <SolidActionButton safety="read" onClick={acceptAndClose}>
            OK
          </SolidActionButton>
        ) : (
          <>
            <Button onClick={close} disabled={submitting}>
              Cancel
            </Button>
            <SolidActionButton
              safety="write"
              onClick={submit}
              disabled={!valid || submitting}
            >
              Upload
            </SolidActionButton>
          </>
        )}
      </DialogActions>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// PEM field helper (paste OR pick from disk)
// ---------------------------------------------------------------------------

interface PemFieldProps {
  label: string;
  placeholder: string;
  accept: string;
  minRows: number;
  value: string;
  onChange: (next: string) => void;
  helperText?: React.ReactNode;
}

function PemField({
  label,
  placeholder,
  accept,
  minRows,
  value,
  onChange,
  helperText,
}: PemFieldProps) {
  const inputId = `pem-file-${label.replace(/\s+/g, "-")}`;
  const handleFile = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    const text = await file.text();
    onChange(text);
    e.target.value = "";
  };
  return (
    <Box>
      <Box
        sx={{
          display: "flex",
          alignItems: "center",
          mb: 0.5,
          gap: 1,
        }}
      >
        <Typography variant="caption" sx={{ color: "text.secondary", flexGrow: 1 }}>
          {label}
        </Typography>
        <Button
          size="small"
          component="label"
          htmlFor={inputId}
          variant="outlined"
        >
          Open file…
          <input
            id={inputId}
            type="file"
            hidden
            accept={accept}
            onChange={handleFile}
          />
        </Button>
      </Box>
      <TextField
        placeholder={placeholder}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        fullWidth
        multiline
        minRows={minRows}
        slotProps={{
          htmlInput: { style: { fontFamily: "monospace", fontSize: "0.75rem" } },
        }}
        helperText={helperText}
      />
    </Box>
  );
}

// ---------------------------------------------------------------------------
// CSR dialogs
// ---------------------------------------------------------------------------

interface CsrBeginDialogProps {
  open: boolean;
  onClose: () => void;
  onSubmitted: (result: TlsCsrBeginResponse) => void;
}

function CsrBeginDialog({ open, onClose, onSubmitted }: CsrBeginDialogProps) {
  const [hostname, setHostname] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const { execute, error, clearError } = useOiAction();

  const close = () => {
    setHostname("");
    clearError();
    onClose();
  };

  const submit = async () => {
    setSubmitting(true);
    try {
      const result = (await execute("/tls/certificates/csr/begin", {
        hostname: hostname.trim(),
        key_type: "ecdsa_p256",
      })) as TlsCsrBeginResponse;
      setHostname("");
      onSubmitted(result);
    } catch {
      // surfaced inline via `error`
    } finally {
      setSubmitting(false);
    }
  };

  const trimmed = hostname.trim();
  const valid = trimmed.length > 0 && !trimmed.includes("*");

  return (
    <Dialog open={open} onClose={close} fullWidth maxWidth="sm">
      <DialogTitle>Generate CSR</DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 1 }}>
          {error && (
            <Alert severity="error" onClose={clearError}>
              {error.message}
            </Alert>
          )}
          <TextField
            autoFocus
            label="Hostname"
            placeholder="e.g. app.example.com"
            value={hostname}
            onChange={(e) => setHostname(e.target.value)}
            fullWidth
            slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
            helperText="Must be a concrete hostname (no wildcards). The runtime generates an ECDSA P-256 keypair on the server; only the CSR is returned. Get the CSR signed by your CA, then come back to upload the signed cert."
          />
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={close} disabled={submitting}>
          Cancel
        </Button>
        <SolidActionButton safety="write" onClick={submit} disabled={!valid || submitting}>
          Generate
        </SolidActionButton>
      </DialogActions>
    </Dialog>
  );
}

interface CsrShowDialogProps {
  open: boolean;
  csrId: number | null;
  csrPem: string;
  onClose: () => void;
}

function CsrShowDialog({ open, csrId, csrPem, onClose }: CsrShowDialogProps) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(csrPem);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // ignore
    }
  };
  const download = () => {
    const blob = new Blob([csrPem], { type: "application/pem-certificate-chain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `csr-${csrId ?? "unknown"}.pem`;
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
  };
  return (
    <Dialog open={open} onClose={onClose} fullWidth maxWidth="md">
      <DialogTitle>
        Pending CSR{csrId !== null ? ` #${csrId}` : ""}
      </DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 1 }}>
          <Alert severity="info">
            Send this CSR to your certificate authority. When you receive the signed
            certificate, upload it back to this row to transition it to active. The
            private key never leaves the runtime.
          </Alert>
          <TextField
            value={csrPem}
            multiline
            minRows={10}
            fullWidth
            slotProps={{
              htmlInput: {
                readOnly: true,
                style: { fontFamily: "monospace", fontSize: "0.75rem" },
              },
            }}
          />
          <Box sx={{ display: "flex", gap: 1, justifyContent: "flex-end" }}>
            <OutlinedActionButton safety="read" onClick={download}>
              Download
            </OutlinedActionButton>
            <OutlinedActionButton safety="read" onClick={copy}>
              {copied ? "Copied" : "Copy"}
            </OutlinedActionButton>
          </Box>
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose}>Close</Button>
      </DialogActions>
    </Dialog>
  );
}

interface CsrUploadCertDialogProps {
  open: boolean;
  cert: TlsCertificate | null;
  onClose: () => void;
  onSubmitted: () => void;
}

function CsrUploadCertDialog({
  open,
  cert,
  onClose,
  onSubmitted,
}: CsrUploadCertDialogProps) {
  const [certPem, setCertPem] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [warnings, setWarnings] = useState<string[] | null>(null);
  const { execute, error, clearError } = useOiAction();

  const reset = () => {
    setCertPem("");
    setWarnings(null);
    clearError();
  };

  const close = () => {
    reset();
    onClose();
  };

  const submit = async () => {
    if (!cert) return;
    setSubmitting(true);
    setWarnings(null);
    try {
      const result = (await execute("/tls/certificates/csr/upload-cert", {
        id: cert.id,
        cert_pem: certPem,
      })) as { warnings?: string[] };
      const warns = result?.warnings ?? [];
      if (warns.length > 0) {
        setWarnings(warns);
      } else {
        reset();
        onSubmitted();
      }
    } catch {
      // surfaced inline via `error`
    } finally {
      setSubmitting(false);
    }
  };

  const acceptAndClose = () => {
    reset();
    onSubmitted();
  };

  const valid = certPem.trim().length > 0;

  return (
    <Dialog open={open} onClose={close} fullWidth maxWidth="md">
      <DialogTitle>
        Upload signed cert{cert ? ` for ${cert.hostname} (#${cert.id})` : ""}
      </DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 1 }}>
          {error && (
            <Alert severity="error" onClose={clearError}>
              {error.message}
            </Alert>
          )}
          <PemField
            label="Signed certificate PEM"
            placeholder="-----BEGIN CERTIFICATE-----..."
            accept=".pem,.crt,.cer"
            minRows={8}
            value={certPem}
            onChange={setCertPem}
            helperText="The runtime checks that this cert's public key matches the CSR's stored private key, plus SAN coverage and validity."
          />
          {warnings && (
            <Alert severity="warning">
              Uploaded with warnings: {warnings.join(", ")}.
            </Alert>
          )}
        </Stack>
      </DialogContent>
      <DialogActions>
        {warnings ? (
          <SolidActionButton safety="read" onClick={acceptAndClose}>
            OK
          </SolidActionButton>
        ) : (
          <>
            <Button onClick={close} disabled={submitting}>
              Cancel
            </Button>
            <SolidActionButton
              safety="write"
              onClick={submit}
              disabled={!valid || submitting}
            >
              Upload
            </SolidActionButton>
          </>
        )}
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
  safety: "write" | "dangerous";
  onClose: () => void;
  onConfirm: () => void;
}

function ConfirmDialog({
  open,
  title,
  body,
  confirmLabel,
  confirmColor,
  safety,
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
        <SolidActionButton
          safety={safety}
          color={confirmColor}
          onClick={onConfirm}
        >
          {confirmLabel}
        </SolidActionButton>
      </DialogActions>
    </Dialog>
  );
}
