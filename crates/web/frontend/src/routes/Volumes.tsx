import AddIcon from "@mui/icons-material/Add";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutline";
import EditIcon from "@mui/icons-material/Edit";
import LinkOffIcon from "@mui/icons-material/LinkOff";
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
import { Link } from "react-router-dom";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useOiAction } from "../hooks/useOiAction";
import { useOiQuery } from "../hooks/useOi";
import type {
  ExportedVolume,
  ExternalMapping,
  HeldVolume,
  SiteVolume,
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

// w[impl routes.volumes]
function MapVolumeDialog({
  open,
  onClose,
  onSuccess,
  existing,
  siteVolumes,
  exportedVolumes,
}: {
  open: boolean;
  onClose: () => void;
  onSuccess: () => void;
  existing?: ExternalMapping;
  siteVolumes: SiteVolume[];
  exportedVolumes: ExportedVolume[];
}) {
  const { execute, loading, error, clearError } = useOiAction();
  const [app, setApp] = useState(existing?.app ?? "");
  const [externalName, setExternalName] = useState(
    existing?.external_name ?? "",
  );
  const [targetKind, setTargetKind] = useState<"site" | "exported">(
    existing?.target_kind ?? "site",
  );
  const [targetApp, setTargetApp] = useState(existing?.target_app ?? "");
  const [targetVolume, setTargetVolume] = useState(
    existing?.target_volume ?? "",
  );
  const [readOnly, setReadOnly] = useState(existing?.read_only ?? false);

  const isRemap = !!existing;

  const handleClose = () => {
    clearError();
    onClose();
  };

  const handleSubmit = async () => {
    try {
      await execute(
        isRemap ? "/volumes/external/remap" : "/volumes/external/map",
        {
          app,
          external_name: externalName,
          target_kind: targetKind,
          ...(targetKind === "exported" ? { target_app: targetApp } : {}),
          target_volume: targetVolume,
          read_only: readOnly,
        },
      );
      onSuccess();
    } catch {
      // displayed via error
    }
  };

  const filteredExported =
    targetApp
      ? exportedVolumes.filter((v) => v.app === targetApp)
      : exportedVolumes;

  const canSubmit =
    !!app &&
    !!externalName &&
    !!targetVolume &&
    (targetKind === "site" || !!targetApp);

  return (
    <Dialog open={open} onClose={handleClose} maxWidth="sm" fullWidth>
      <DialogTitle>
        {isRemap ? "Remap External Volume" : "Map External Volume"}
      </DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 0.5 }}>
          {error && <OiErrorAlert error={error} />}
          <TextField
            label="App"
            size="small"
            value={app}
            onChange={(e) => setApp(e.target.value)}
            inputProps={{ style: { fontFamily: "monospace" } }}
            disabled={isRemap}
            autoFocus={!isRemap}
          />
          <TextField
            label="External volume name"
            size="small"
            value={externalName}
            onChange={(e) => setExternalName(e.target.value)}
            inputProps={{ style: { fontFamily: "monospace" } }}
            disabled={isRemap}
            autoFocus={isRemap}
          />
          <FormControl>
            <FormLabel>Target</FormLabel>
            <RadioGroup
              row
              value={targetKind}
              onChange={(e) => {
                setTargetKind(e.target.value as typeof targetKind);
                setTargetVolume("");
              }}
            >
              <FormControlLabel
                value="site"
                control={<Radio size="small" />}
                label="Site volume"
              />
              <FormControlLabel
                value="exported"
                control={<Radio size="small" />}
                label="Exported app volume"
              />
            </RadioGroup>
          </FormControl>
          {targetKind === "site" && (
            siteVolumes.length > 0 ? (
              <FormControl size="small">
                <InputLabel>Site volume</InputLabel>
                <Select
                  label="Site volume"
                  value={targetVolume}
                  onChange={(e) => setTargetVolume(e.target.value)}
                  sx={{ fontFamily: "monospace" }}
                >
                  {siteVolumes.map((v) => (
                    <MenuItem
                      key={v.name}
                      value={v.name}
                      sx={{ fontFamily: "monospace" }}
                    >
                      {v.name}
                      <Typography
                        component="span"
                        variant="caption"
                        color="text.secondary"
                        sx={{ ml: 1 }}
                      >
                        {v.kind}
                      </Typography>
                    </MenuItem>
                  ))}
                </Select>
              </FormControl>
            ) : (
              <TextField
                label="Site volume name"
                size="small"
                value={targetVolume}
                onChange={(e) => setTargetVolume(e.target.value)}
                inputProps={{ style: { fontFamily: "monospace" } }}
                helperText="No site volumes found — enter the name manually."
              />
            )
          )}
          {targetKind === "exported" && (
            <>
              <TextField
                label="Source app"
                size="small"
                value={targetApp}
                onChange={(e) => {
                  setTargetApp(e.target.value);
                  setTargetVolume("");
                }}
                inputProps={{ style: { fontFamily: "monospace" } }}
              />
              {filteredExported.length > 0 ? (
                <FormControl size="small">
                  <InputLabel>Exported volume</InputLabel>
                  <Select
                    label="Exported volume"
                    value={targetVolume}
                    onChange={(e) => setTargetVolume(e.target.value)}
                    sx={{ fontFamily: "monospace" }}
                  >
                    {filteredExported.map((v) => (
                      <MenuItem
                        key={v.volume_name}
                        value={v.volume_name}
                        sx={{ fontFamily: "monospace" }}
                      >
                        {v.volume_name}
                        {v.description && (
                          <Typography
                            component="span"
                            variant="caption"
                            color="text.secondary"
                            sx={{ ml: 1 }}
                          >
                            {v.description}
                          </Typography>
                        )}
                      </MenuItem>
                    ))}
                  </Select>
                </FormControl>
              ) : (
                <TextField
                  label="Exported volume name"
                  size="small"
                  value={targetVolume}
                  onChange={(e) => setTargetVolume(e.target.value)}
                  inputProps={{ style: { fontFamily: "monospace" } }}
                />
              )}
            </>
          )}
          <FormControlLabel
            control={
              <Checkbox
                checked={readOnly}
                onChange={(e) => setReadOnly(e.target.checked)}
                size="small"
              />
            }
            label="Mount read-only"
          />
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
          {loading
            ? isRemap
              ? "Remapping…"
              : "Mapping…"
            : isRemap
              ? "Remap"
              : "Map"}
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
    data: heldVols,
    loading: heldLoading,
    error: heldError,
    refetch: refetchHeld,
  } = useOiQuery<HeldVolume[]>("/volumes/held/list", {});

  const { execute, error: actionError } = useOiAction();

  const [createOpen, setCreateOpen] = useState(false);
  const [mapOpen, setMapOpen] = useState(false);
  const [remapTarget, setRemapTarget] = useState<ExternalMapping | null>(null);

  const refreshAll = () => {
    refetchSite();
    refetchExported();
    refetchMappings();
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
    siteLoading || exportedLoading || mappingsLoading || heldLoading;

  return (
    <Box sx={{ p: 3, maxWidth: 900, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          Volumes
        </Typography>
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
                      <TableCell width={40} />
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
                        <TableCell align="right" sx={{ px: 0.5 }}>
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
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </TableContainer>
            ))}
        </Box>

        <Divider />

        {/* External Volume Mappings */}
        <Box>
          <Box sx={{ display: "flex", alignItems: "center", mb: 1, gap: 1 }}>
            <Typography variant="subtitle1" sx={{ fontWeight: 600, flexGrow: 1 }}>
              External Volume Mappings
            </Typography>
            <Button
              size="small"
              startIcon={<AddIcon />}
              onClick={() => setMapOpen(true)}
            >
              Add
            </Button>
          </Box>
          {mappingsError && <OiErrorAlert error={mappingsError} />}
          {mappingsLoading && !mappings && <CircularProgress size={20} />}
          {mappings &&
            (mappings.length === 0 ? (
              <Typography color="text.secondary" variant="body2">
                No mappings.
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
                    {mappings.map((m) => (
                      <TableRow key={`${m.app}/${m.external_name}`}>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          <Link to={`/apps/${m.app}`}>{m.app}</Link>
                        </TableCell>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          {m.external_name}
                        </TableCell>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          {m.target_kind === "exported"
                            ? `${m.target_app}/${m.target_volume}`
                            : `_site/${m.target_volume}`}
                          {m.read_only && (
                            <Chip
                              label="ro"
                              size="small"
                              variant="outlined"
                              sx={{ ml: 1 }}
                            />
                          )}
                        </TableCell>
                        <TableCell align="right" sx={{ px: 0.5, whiteSpace: "nowrap" }}>
                          <Tooltip title="Remap">
                            <IconButton
                              size="small"
                              onClick={() => setRemapTarget(m)}
                            >
                              <EditIcon sx={{ fontSize: 16 }} />
                            </IconButton>
                          </Tooltip>
                          <Tooltip title="Unmap">
                            <IconButton
                              size="small"
                              onClick={() =>
                                void unmapVolume(m.app, m.external_name)
                              }
                            >
                              <LinkOffIcon sx={{ fontSize: 16 }} />
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
                      <TableCell width={40} />
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
                        <TableCell align="right" sx={{ px: 0.5 }}>
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

      {(mapOpen || remapTarget != null) && (
        <MapVolumeDialog
          key={
            remapTarget
              ? `remap:${remapTarget.app}/${remapTarget.external_name}`
              : "new"
          }
          open={mapOpen || remapTarget != null}
          onClose={() => {
            setMapOpen(false);
            setRemapTarget(null);
          }}
          onSuccess={() => {
            refetchMappings();
            setMapOpen(false);
            setRemapTarget(null);
          }}
          existing={remapTarget ?? undefined}
          siteVolumes={siteVols ?? []}
          exportedVolumes={exportedVols ?? []}
        />
      )}
    </Box>
  );
}
