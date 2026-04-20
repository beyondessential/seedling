import AddIcon from "@mui/icons-material/Add";
import CameraAltIcon from "@mui/icons-material/CameraAlt";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutline";
import EditIcon from "@mui/icons-material/Edit";
import LinkOffIcon from "@mui/icons-material/LinkOff";
import RefreshIcon from "@mui/icons-material/Refresh";
import TerminalIcon from "@mui/icons-material/Terminal";
import UpgradeIcon from "@mui/icons-material/Upgrade";
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
  Divider,
  FormControl,
  FormControlLabel,
  FormLabel,
  IconButton,
  InputLabel,
  List,
  ListItem,
  ListItemButton,
  ListItemIcon,
  ListItemText,
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
import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { MapVolumeDialog } from "../components/MapVolumeDialog";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { PromoteSnapshotDialog } from "../components/PromoteSnapshotDialog";
import { useSessionContext } from "../components/SessionProvider";
import { SnapshotVolumeDialog } from "../components/SnapshotVolumeDialog";
import { useOiAction } from "../hooks/useOiAction";
import { useOiQuery } from "../hooks/useOi";
import type {
  DeclaredExternalVolume,
  ExportedVolume,
  ExternalMapping,
  HeldVolume,
  SiteVolume,
  VolumeRef,
} from "../lib/types";

// w[impl routes.volumes]
function CreateSiteVolumeDialog({
  open,
  onClose,
  onSuccess,
  siteVolumes,
  exportedVolumes,
}: {
  open: boolean;
  onClose: () => void;
  onSuccess: () => void;
  siteVolumes: SiteVolume[];
  exportedVolumes: ExportedVolume[];
}) {
  const { execute, loading, error, clearError } = useOiAction();
  const [name, setName] = useState("");
  const [kind, setKind] = useState<"managed" | "bind" | "snapshot">("managed");
  const [hostPath, setHostPath] = useState("");
  const [source, setSource] = useState("");

  const handleClose = () => {
    clearError();
    setName("");
    setKind("managed");
    setHostPath("");
    setSource("");
    onClose();
  };

  const handleSubmit = async () => {
    try {
      if (kind === "snapshot") {
        await execute("/volumes/site/snapshot", { name, source });
      } else {
        await execute("/volumes/site/create", {
          name,
          kind,
          ...(kind === "bind" ? { host_path: hostPath } : {}),
        });
      }
      onSuccess();
      handleClose();
    } catch {
      // displayed via error
    }
  };

  const snapshotOptions = [
    ...siteVolumes
      .filter((v) => v.kind !== "snapshot")
      .map((v) => ({ value: `_site/${v.name}`, label: `_site/${v.name}` })),
    ...exportedVolumes.map((v) => ({
      value: `${v.app}/${v.volume_name}`,
      label: `${v.app}/${v.volume_name}${v.description ? ` — ${v.description}` : ""}`,
    })),
  ];

  const canSubmit =
    !!name &&
    (kind === "managed" ||
      (kind === "bind" && !!hostPath) ||
      (kind === "snapshot" && !!source));

  return (
    <Dialog open={open} onClose={handleClose} maxWidth="sm" fullWidth>
      <DialogTitle>New Site Volume</DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 0.5 }}>
          {error && <OiErrorAlert error={error} />}
          <TextField
            label="Name"
            size="small"
            value={name}
            onChange={(e) => setName(e.target.value)}
            inputProps={{ style: { fontFamily: "monospace" } }}
            autoFocus
          />
          <FormControl>
            <FormLabel>Kind</FormLabel>
            <RadioGroup
              row
              value={kind}
              onChange={(e) => setKind(e.target.value as typeof kind)}
            >
              <FormControlLabel
                value="managed"
                control={<Radio size="small" />}
                label="Managed"
              />
              <FormControlLabel
                value="bind"
                control={<Radio size="small" />}
                label="Bind mount"
              />
              <FormControlLabel
                value="snapshot"
                control={<Radio size="small" />}
                label="Snapshot"
              />
            </RadioGroup>
          </FormControl>
          {kind === "bind" && (
            <TextField
              label="Host path"
              size="small"
              value={hostPath}
              onChange={(e) => setHostPath(e.target.value)}
              inputProps={{ style: { fontFamily: "monospace" } }}
              placeholder="/data/mypath"
            />
          )}
          {kind === "snapshot" && (
            snapshotOptions.length > 0 ? (
              <FormControl size="small">
                <InputLabel>Source volume</InputLabel>
                <Select
                  label="Source volume"
                  value={source}
                  onChange={(e) => setSource(e.target.value)}
                  sx={{ fontFamily: "monospace" }}
                >
                  {snapshotOptions.map((opt) => (
                    <MenuItem
                      key={opt.value}
                      value={opt.value}
                      sx={{ fontFamily: "monospace" }}
                    >
                      {opt.label}
                    </MenuItem>
                  ))}
                </Select>
              </FormControl>
            ) : (
              <TextField
                label="Source volume"
                size="small"
                value={source}
                onChange={(e) => setSource(e.target.value)}
                inputProps={{ style: { fontFamily: "monospace" } }}
                placeholder="_site/name or app/volume"
                helperText="No site volumes or exported app volumes found — enter manually."
              />
            )
          )}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={handleClose} disabled={loading}>
          Cancel
        </Button>
        <Button
          variant="contained"
          onClick={() => void handleSubmit()}
          disabled={loading || !canSubmit}
        >
          {loading ? "Creating…" : "Create"}
        </Button>
      </DialogActions>
    </Dialog>
  );
}


/// Opens a single shell session with one or more volumes mounted side-by-side.
/// Operators land in /mnt when multiple volumes are selected (the backend
/// sets workdir to the single mount point when only one is chosen).
function MultiVolumeShellDialog({
  open,
  onClose,
  siteVols,
  exportedVols,
  heldVols,
  onOpen,
}: {
  open: boolean;
  onClose: () => void;
  siteVols: SiteVolume[];
  exportedVols: ExportedVolume[];
  heldVols: HeldVolume[];
  onOpen: (volumes: VolumeRef[], label: string) => void;
}) {
  // Selection is keyed by a stable identifier string — "site:<name>",
  // "app:<app>/<vol>", "held:<id>" — so a selected entry survives a
  // re-render that reorders the lists.
  const [selected, setSelected] = useState<Set<string>>(new Set());

  const toggle = (key: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key); else next.add(key);
      return next;
    });
  };

  const { refs, label } = useMemo(() => {
    const refs: VolumeRef[] = [];
    const labels: string[] = [];
    for (const v of siteVols) {
      const key = `site:${v.name}`;
      if (selected.has(key)) {
        refs.push({ kind: "site", name: v.name });
        labels.push(v.name);
      }
    }
    for (const v of exportedVols) {
      const key = `app:${v.app}/${v.volume_name}`;
      if (selected.has(key)) {
        refs.push({ kind: "app", app: v.app, volume: v.volume_name });
        labels.push(`${v.app}/${v.volume_name}`);
      }
    }
    for (const h of heldVols) {
      const key = `held:${h.id}`;
      if (selected.has(key)) {
        refs.push({ kind: "held", id: h.id });
        labels.push(`held:${h.app}/${h.display_name}`);
      }
    }
    const label =
      labels.length === 0
        ? ""
        : labels.length === 1
          ? labels[0]
          : `${labels.length} volumes`;
    return { refs, label };
  }, [selected, siteVols, exportedVols, heldVols]);

  const handleOpen = () => {
    if (refs.length === 0) return;
    onOpen(refs, label);
    setSelected(new Set());
    onClose();
  };

  const handleClose = () => {
    setSelected(new Set());
    onClose();
  };

  const sectionLabel = (title: string, count: number) =>
    count === 0 ? `${title} (none)` : title;

  return (
    <Dialog open={open} onClose={handleClose} maxWidth="sm" fullWidth>
      <DialogTitle>Open shell over volumes</DialogTitle>
      <DialogContent>
        <Typography variant="body2" color="text.secondary" sx={{ mb: 2 }}>
          Mount multiple volumes side-by-side under <code>/mnt</code> so you
          can <code>cp</code>, <code>rsync</code>, diff, or recover data
          across them from a single shell.
        </Typography>

        <Box sx={{ mb: 2 }}>
          <Typography variant="subtitle2" sx={{ mb: 0.5 }}>
            {sectionLabel("Site volumes", siteVols.length)}
          </Typography>
          <List dense disablePadding>
            {siteVols.map((v) => {
              const key = `site:${v.name}`;
              return (
                <ListItem key={key} disablePadding>
                  <ListItemButton onClick={() => toggle(key)} dense>
                    <ListItemIcon sx={{ minWidth: 32 }}>
                      <Checkbox
                        edge="start"
                        size="small"
                        checked={selected.has(key)}
                        tabIndex={-1}
                        disableRipple
                      />
                    </ListItemIcon>
                    <ListItemText
                      primary={v.name}
                      primaryTypographyProps={{ sx: { fontFamily: "monospace" } }}
                      secondary={v.kind}
                    />
                  </ListItemButton>
                </ListItem>
              );
            })}
          </List>
        </Box>

        <Box sx={{ mb: 2 }}>
          <Typography variant="subtitle2" sx={{ mb: 0.5 }}>
            {sectionLabel("Exported app volumes", exportedVols.length)}
          </Typography>
          <List dense disablePadding>
            {exportedVols.map((v) => {
              const key = `app:${v.app}/${v.volume_name}`;
              return (
                <ListItem key={key} disablePadding>
                  <ListItemButton onClick={() => toggle(key)} dense>
                    <ListItemIcon sx={{ minWidth: 32 }}>
                      <Checkbox
                        edge="start"
                        size="small"
                        checked={selected.has(key)}
                        tabIndex={-1}
                        disableRipple
                      />
                    </ListItemIcon>
                    <ListItemText
                      primary={`${v.app}/${v.volume_name}`}
                      primaryTypographyProps={{ sx: { fontFamily: "monospace" } }}
                      secondary={v.description ?? undefined}
                    />
                  </ListItemButton>
                </ListItem>
              );
            })}
          </List>
        </Box>

        <Box>
          <Typography variant="subtitle2" sx={{ mb: 0.5 }}>
            {sectionLabel("Held volumes", heldVols.length)}
          </Typography>
          <List dense disablePadding>
            {heldVols.map((h) => {
              const key = `held:${h.id}`;
              return (
                <ListItem key={key} disablePadding>
                  <ListItemButton onClick={() => toggle(key)} dense>
                    <ListItemIcon sx={{ minWidth: 32 }}>
                      <Checkbox
                        edge="start"
                        size="small"
                        checked={selected.has(key)}
                        tabIndex={-1}
                        disableRipple
                      />
                    </ListItemIcon>
                    <ListItemText
                      primary={`${h.app}/${h.display_name}`}
                      primaryTypographyProps={{ sx: { fontFamily: "monospace" } }}
                      secondary={h.reason}
                    />
                  </ListItemButton>
                </ListItem>
              );
            })}
          </List>
        </Box>
      </DialogContent>
      <DialogActions>
        <Button onClick={handleClose}>Cancel</Button>
        <Button
          variant="contained"
          startIcon={<TerminalIcon />}
          onClick={handleOpen}
          disabled={refs.length === 0}
        >
          Open shell {refs.length > 0 ? `(${refs.length})` : ""}
        </Button>
      </DialogActions>
    </Dialog>
  );
}


// w[impl routes.volumes]
export default function Volumes() {
  const {
    data: siteVols,
    loading: siteLoading,
    error: siteError,
    refetch: refetchSite,
  } = useOiQuery<SiteVolume[]>("/volumes/site/list", {});
  const {
    data: exportedVols,
    loading: exportedLoading,
    error: exportedError,
    refetch: refetchExported,
  } = useOiQuery<ExportedVolume[]>("/volumes/exported/list", {});
  const {
    data: mappings,
    loading: mappingsLoading,
    error: mappingsError,
    refetch: refetchMappings,
  } = useOiQuery<ExternalMapping[]>("/volumes/external/list", {});
  const {
    data: declared,
    loading: declaredLoading,
    error: declaredError,
    refetch: refetchDeclared,
  } = useOiQuery<DeclaredExternalVolume[]>("/volumes/external/declared", {});
  const {
    data: heldVols,
    loading: heldLoading,
    error: heldError,
    refetch: refetchHeld,
  } = useOiQuery<HeldVolume[]>("/volumes/held/list", {});

  const { execute, error: actionError } = useOiAction();
  const { openVolumeShell } = useSessionContext();

  const [createOpen, setCreateOpen] = useState(false);
  const [mapOpen, setMapOpen] = useState(false);
  const [shellPickerOpen, setShellPickerOpen] = useState(false);
  const [snapshotTarget, setSnapshotTarget] = useState<
    { source: string; label: string } | null
  >(null);
  const [promoteTarget, setPromoteTarget] = useState<string | null>(null);
  const [remapTarget, setRemapTarget] = useState<ExternalMapping | null>(null);
  const [prefillTarget, setPrefillTarget] = useState<{ app: string; name: string } | null>(null);

  const refreshAll = () => {
    refetchSite();
    refetchExported();
    refetchMappings();
    refetchDeclared();
    refetchHeld();
  };

  const deleteSiteVol = async (name: string) => {
    await execute("/volumes/site/delete", { name });
    refetchSite();
  };

  const unmapVolume = async (app: string, external_name: string) => {
    await execute("/volumes/external/unmap", { app, external_name });
    refetchMappings();
  };

  const confirmDeleteHeld = async (id: string) => {
    await execute("/volumes/held/delete", { id });
    refetchHeld();
  };

  const anyLoading =
    siteLoading || exportedLoading || mappingsLoading || declaredLoading || heldLoading;

  return (
    <Box sx={{ p: 3, maxWidth: 900, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          Volumes
        </Typography>
        <Button
          size="small"
          startIcon={<TerminalIcon />}
          onClick={() => setShellPickerOpen(true)}
          disabled={
            (siteVols?.length ?? 0) === 0 &&
            (exportedVols?.length ?? 0) === 0 &&
            (heldVols?.length ?? 0) === 0
          }
        >
          Open shell…
        </Button>
        <Tooltip title="Refresh">
          <span>
            <IconButton onClick={refreshAll} disabled={anyLoading} size="small">
              <RefreshIcon />
            </IconButton>
          </span>
        </Tooltip>
      </Box>

      {actionError && (
        <Alert severity="error" sx={{ mb: 2 }}>
          {actionError.message}
        </Alert>
      )}

      <Stack spacing={4}>
        {/* Site Volumes */}
        <Box>
          <Box sx={{ display: "flex", alignItems: "center", mb: 1, gap: 1 }}>
            <Typography variant="subtitle1" sx={{ fontWeight: 600, flexGrow: 1 }}>
              Site Volumes
            </Typography>
            <Button
              size="small"
              startIcon={<AddIcon />}
              onClick={() => setCreateOpen(true)}
            >
              New
            </Button>
          </Box>
          {siteError && <OiErrorAlert error={siteError} />}
          {siteLoading && !siteVols && <CircularProgress size={20} />}
          {siteVols &&
            (siteVols.length === 0 ? (
              <Typography color="text.secondary" variant="body2">
                No site volumes.
              </Typography>
            ) : (
              <TableContainer component={Paper} variant="outlined">
                <Table size="small">
                  <TableHead>
                    <TableRow>
                      <TableCell>Name</TableCell>
                      <TableCell width={90}>Kind</TableCell>
                      <TableCell>Info</TableCell>
                      <TableCell width={160}>Created</TableCell>
                      <TableCell width={140} />
                    </TableRow>
                  </TableHead>
                  <TableBody>
                    {siteVols.map((v) => (
                      <TableRow key={v.name}>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          {v.name}
                        </TableCell>
                        <TableCell>
                          <Chip label={v.kind} size="small" variant="outlined" />
                        </TableCell>
                        <TableCell
                          sx={{ fontFamily: "monospace", color: "text.secondary" }}
                        >
                          {v.host_path ?? v.source ?? "—"}
                        </TableCell>
                        <TableCell sx={{ color: "text.secondary" }}>
                          {new Date(v.created_at).toLocaleString()}
                        </TableCell>
                        {/* w[volumes.shell-ui] */}
                        <TableCell align="right" sx={{ px: 0.5, whiteSpace: "nowrap" }}>
                          <Tooltip title="Open shell">
                            <IconButton
                              size="small"
                              onClick={() => openVolumeShell([{ kind: "site", name: v.name }], v.name)}
                            >
                              <TerminalIcon sx={{ fontSize: 16 }} />
                            </IconButton>
                          </Tooltip>
                          <Tooltip title="Snapshot">
                            <IconButton
                              size="small"
                              onClick={() =>
                                setSnapshotTarget({
                                  source: `_site/${v.name}`,
                                  label: v.name,
                                })
                              }
                            >
                              <CameraAltIcon sx={{ fontSize: 16 }} />
                            </IconButton>
                          </Tooltip>
                          {v.kind === "snapshot" && (
                            <Tooltip title="Promote to read-write volume">
                              <IconButton
                                size="small"
                                onClick={() => setPromoteTarget(v.name)}
                              >
                                <UpgradeIcon sx={{ fontSize: 16 }} />
                              </IconButton>
                            </Tooltip>
                          )}
                          <Tooltip title="Delete">
                            <IconButton
                              size="small"
                              onClick={() => void deleteSiteVol(v.name)}
                            >
                              <DeleteOutlineIcon sx={{ fontSize: 16 }} />
                            </IconButton>
                          </Tooltip>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </TableContainer>
            ))}
        </Box>

        <Divider />

        {/* App Exports */}
        <Box>
          <Typography variant="subtitle1" sx={{ fontWeight: 600, mb: 1 }}>
            App Exports
          </Typography>
          {exportedError && <OiErrorAlert error={exportedError} />}
          {exportedLoading && !exportedVols && <CircularProgress size={20} />}
          {exportedVols &&
            (exportedVols.length === 0 ? (
              <Typography color="text.secondary" variant="body2">
                No exported volumes.
              </Typography>
            ) : (
              <TableContainer component={Paper} variant="outlined">
                <Table size="small">
                  <TableHead>
                    <TableRow>
                      <TableCell>App</TableCell>
                      <TableCell>Volume</TableCell>
                      <TableCell>Description</TableCell>
                      <TableCell width={80} />
                    </TableRow>
                  </TableHead>
                  <TableBody>
                    {exportedVols.map((v) => (
                      <TableRow key={`${v.app}/${v.volume_name}`}>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          <Link to={`/apps/${v.app}`}>{v.app}</Link>
                        </TableCell>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          {v.volume_name}
                        </TableCell>
                        <TableCell sx={{ color: "text.secondary" }}>
                          {v.description ?? "—"}
                        </TableCell>
                        <TableCell align="right" sx={{ px: 0.5, whiteSpace: "nowrap" }}>
                          <Tooltip title="Open shell">
                            <IconButton
                              size="small"
                              onClick={() =>
                                openVolumeShell(
                                  [{ kind: "app", app: v.app, volume: v.volume_name }],
                                  `${v.app}/${v.volume_name}`,
                                )
                              }
                            >
                              <TerminalIcon sx={{ fontSize: 16 }} />
                            </IconButton>
                          </Tooltip>
                          <Tooltip title="Snapshot">
                            <IconButton
                              size="small"
                              onClick={() =>
                                setSnapshotTarget({
                                  source: `${v.app}/${v.volume_name}`,
                                  label: `${v.app}/${v.volume_name}`,
                                })
                              }
                            >
                              <CameraAltIcon sx={{ fontSize: 16 }} />
                            </IconButton>
                          </Tooltip>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </TableContainer>
            ))}
        </Box>

        <Divider />

        {/* External Volume Requests */}
        <Box>
          <Box sx={{ display: "flex", alignItems: "center", mb: 1, gap: 1 }}>
            <Typography variant="subtitle1" sx={{ fontWeight: 600, flexGrow: 1 }}>
              External Volume Requests
            </Typography>
            <Button size="small" startIcon={<AddIcon />} onClick={() => setMapOpen(true)}>
              Map
            </Button>
          </Box>
          {declaredError && <OiErrorAlert error={declaredError} />}
          {mappingsError && <OiErrorAlert error={mappingsError} />}
          {(declaredLoading || mappingsLoading) && !declared && <CircularProgress size={20} />}
          {declared && (
            declared.length === 0 ? (
              <Typography color="text.secondary" variant="body2">
                No external volume requests across registered apps.
              </Typography>
            ) : (
              <TableContainer component={Paper} variant="outlined">
                <Table size="small">
                  <TableHead>
                    <TableRow>
                      <TableCell>App</TableCell>
                      <TableCell>External Volume</TableCell>
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
                          <TableCell sx={{ fontFamily: "monospace" }}>{d.name}</TableCell>
                          <TableCell sx={{ fontFamily: "monospace" }}>
                            {mapping ? (
                              <>
                                {mapping.target_kind === "exported"
                                  ? `${mapping.target_app}/${mapping.target_volume}`
                                  : `_site/${mapping.target_volume}`}
                                {mapping.read_only && (
                                  <Chip label="ro" size="small" variant="outlined" sx={{ ml: 1 }} />
                                )}
                              </>
                            ) : (
                              <Typography variant="caption" color="warning.main">unmapped</Typography>
                            )}
                          </TableCell>
                          <TableCell align="right" sx={{ px: 0.5, whiteSpace: "nowrap" }}>
                            {mapping ? (
                              <>
                                <Tooltip title="Remap">
                                  <IconButton size="small" onClick={() => setRemapTarget(mapping)}>
                                    <EditIcon sx={{ fontSize: 16 }} />
                                  </IconButton>
                                </Tooltip>
                                <Tooltip title="Unmap">
                                  <IconButton size="small" onClick={() => void unmapVolume(d.app, d.name)}>
                                    <LinkOffIcon sx={{ fontSize: 16 }} />
                                  </IconButton>
                                </Tooltip>
                              </>
                            ) : (
                              <Button
                                size="small"
                                onClick={() => setPrefillTarget({ app: d.app, name: d.name })}
                              >
                                Map
                              </Button>
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

        {/* Held Volumes — only show if any exist */}
        {heldVols && heldVols.length > 0 && (
          <>
            <Divider />
            <Box>
              <Typography variant="subtitle1" sx={{ fontWeight: 600, mb: 1 }}>
                Held Volumes
              </Typography>
              {heldError && <OiErrorAlert error={heldError} />}
              <TableContainer component={Paper} variant="outlined">
                <Table size="small">
                  <TableHead>
                    <TableRow>
                      <TableCell>App</TableCell>
                      <TableCell>Volume</TableCell>
                      <TableCell>Reason</TableCell>
                      <TableCell width={160}>Held since</TableCell>
                      <TableCell width={80} />
                    </TableRow>
                  </TableHead>
                  <TableBody>
                    {heldVols.map((h) => (
                      <TableRow key={h.id}>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          <Link to={`/apps/${h.app}`}>{h.app}</Link>
                        </TableCell>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          {h.display_name}
                        </TableCell>
                        <TableCell sx={{ color: "text.secondary" }}>
                          {h.reason}
                        </TableCell>
                        <TableCell sx={{ color: "text.secondary" }}>
                          {new Date(h.held_at).toLocaleString()}
                        </TableCell>
                        <TableCell align="right" sx={{ px: 0.5, whiteSpace: "nowrap" }}>
                          <Tooltip title="Open shell">
                            <IconButton
                              size="small"
                              onClick={() =>
                                openVolumeShell(
                                  [{ kind: "held", id: h.id }],
                                  `held: ${h.app}/${h.display_name}`,
                                )
                              }
                            >
                              <TerminalIcon sx={{ fontSize: 16 }} />
                            </IconButton>
                          </Tooltip>
                          <Tooltip title="Confirm delete">
                            <IconButton
                              size="small"
                              onClick={() => void confirmDeleteHeld(h.id)}
                            >
                              <DeleteOutlineIcon sx={{ fontSize: 16 }} />
                            </IconButton>
                          </Tooltip>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </TableContainer>
            </Box>
          </>
        )}
      </Stack>

      <CreateSiteVolumeDialog
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        onSuccess={() => {
          refetchSite();
          refetchExported();
        }}
        siteVolumes={siteVols ?? []}
        exportedVolumes={exportedVols ?? []}
      />

      <MultiVolumeShellDialog
        open={shellPickerOpen}
        onClose={() => setShellPickerOpen(false)}
        siteVols={siteVols ?? []}
        exportedVols={exportedVols ?? []}
        heldVols={heldVols ?? []}
        onOpen={(volumes, label) => openVolumeShell(volumes, label)}
      />

      {snapshotTarget && (
        <SnapshotVolumeDialog
          key={snapshotTarget.source}
          source={snapshotTarget.source}
          sourceLabel={snapshotTarget.label}
          onClose={() => setSnapshotTarget(null)}
          onSuccess={() => {
            refetchSite();
          }}
        />
      )}

      {promoteTarget && (
        <PromoteSnapshotDialog
          key={promoteTarget}
          source={promoteTarget}
          onClose={() => setPromoteTarget(null)}
          onSuccess={() => {
            refetchSite();
          }}
        />
      )}

      {(mapOpen || remapTarget != null || prefillTarget != null) && (
        <MapVolumeDialog
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
          existing={remapTarget ?? undefined}
          prefill={prefillTarget ?? undefined}
        />
      )}
    </Box>
  );
}
