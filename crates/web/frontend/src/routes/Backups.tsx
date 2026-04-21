import AddIcon from "@mui/icons-material/Add";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutline";
import HistoryIcon from "@mui/icons-material/History";
import PlayArrowIcon from "@mui/icons-material/PlayArrow";
import RefreshIcon from "@mui/icons-material/Refresh";
import RestoreIcon from "@mui/icons-material/Restore";
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
  IconButton,
  InputLabel,
  MenuItem,
  OutlinedInput,
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
import { useState } from "react";
import { Link } from "react-router-dom";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useOiQuery } from "../hooks/useOi";
import { useOiAction } from "../hooks/useOiAction";
import type {
  AppSummary,
  BackupApp,
  BackupRunResult,
  BackupStrategy,
  ExportedVolume,
  SiteVolume,
} from "../lib/types";
import { BACKUP_SCHEDULES } from "../lib/types";

// ── helpers ──────────────────────────────────────────────────────────────────

function snapshotId(item: unknown): string | null {
  if (typeof item === "string") return item;
  if (typeof item === "object" && item !== null) {
    const obj = item as Record<string, unknown>;
    for (const key of ["id", "snapshot_id", "name", "key"]) {
      if (typeof obj[key] === "string") return obj[key] as string;
    }
  }
  return null;
}

function volumeOptions(siteVols: SiteVolume[], exportedVols: ExportedVolume[]) {
  return [
    ...siteVols
      .filter((v) => v.kind !== "snapshot")
      .map((v) => `_site/${v.name}`),
    ...exportedVols.map((v) => `${v.app}/${v.volume_name}`),
  ];
}

// ── Snapshots dialog ──────────────────────────────────────────────────────────

function SnapshotsDialog({
  strategy,
  onClose,
}: {
  strategy: BackupStrategy;
  onClose: () => void;
}) {
  const [volume, setVolume] = useState(strategy.volumes[0] ?? "");
  const [restoredVolume, setRestoredVolume] = useState<string | null>(null);

  const { data: snapshots, loading, error, refetch } = useOiQuery<unknown>(
    "/backups/snapshots/list",
    { strategy: strategy.name, volume },
  );

  const { execute: doRestore, loading: restoring, error: restoreError } = useOiAction();

  const reload = () => { refetch(); };

  const handleRestore = async (snapshot: string) => {
    const result = await doRestore("/backups/restore", {
      strategy: strategy.name,
      volume,
      snapshot,
    }) as { site_volume: string } | null;
    if (result?.site_volume) setRestoredVolume(result.site_volume);
  };

  const snapshotList = Array.isArray(snapshots) ? snapshots : null;

  return (
    <Dialog open onClose={onClose} maxWidth="md" fullWidth>
      <DialogTitle sx={{ display: "flex", alignItems: "center", gap: 1 }}>
        <HistoryIcon fontSize="small" />
        Snapshots — {strategy.name}
      </DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 0.5 }}>
          {strategy.volumes.length > 1 && (
            <FormControl size="small" sx={{ minWidth: 260 }}>
              <InputLabel>Volume</InputLabel>
              <Select
                label="Volume"
                value={volume}
                onChange={(e) => setVolume(e.target.value)}
                sx={{ fontFamily: "monospace" }}
              >
                {strategy.volumes.map((v) => (
                  <MenuItem key={v} value={v} sx={{ fontFamily: "monospace" }}>{v}</MenuItem>
                ))}
              </Select>
            </FormControl>
          )}

          {strategy.volumes.length === 1 && (
            <Typography variant="body2" sx={{ fontFamily: "monospace", color: "text.secondary" }}>
              {volume}
            </Typography>
          )}

          {restoredVolume && (
            <Alert severity="success" onClose={() => setRestoredVolume(null)}>
              Restored to site volume <strong>{restoredVolume}</strong>.{" "}
              <Link to="/volumes">View in Volumes →</Link>
            </Alert>
          )}

          {restoreError && <OiErrorAlert error={restoreError} />}
          {error && <OiErrorAlert error={error} />}

          {loading && <CircularProgress size={24} />}

          {snapshots !== null && !loading && (
            snapshotList ? (
              snapshotList.length === 0 ? (
                <Typography color="text.secondary" variant="body2">No snapshots found.</Typography>
              ) : (
                <TableContainer component={Paper} variant="outlined">
                  <Table size="small">
                    <TableHead>
                      <TableRow>
                        {/* Action column first so the restore button is
                            visible at the start of every row, even when
                            the snapshot detail fields are wide enough to
                            require horizontal scrolling. */}
                        <TableCell width={48} />
                        {Object.keys(snapshotList[0] as object).map((k) => (
                          <TableCell key={k}>{k}</TableCell>
                        ))}
                      </TableRow>
                    </TableHead>
                    <TableBody>
                      {snapshotList.map((item, i) => {
                        const id = snapshotId(item);
                        const fields = typeof item === "object" && item !== null
                          ? Object.entries(item as Record<string, unknown>)
                          : [["value", String(item)]];
                        return (
                          <TableRow key={i}>
                            <TableCell sx={{ px: 0.5 }}>
                              {id ? (
                                <Tooltip title={`Restore snapshot "${id}"`}>
                                  <span>
                                    <IconButton
                                      size="small"
                                      disabled={restoring}
                                      onClick={() => void handleRestore(id)}
                                    >
                                      <RestoreIcon sx={{ fontSize: 16 }} />
                                    </IconButton>
                                  </span>
                                </Tooltip>
                              ) : null}
                            </TableCell>
                            {fields.map(([k, v]) => (
                              <TableCell key={k} sx={{ fontFamily: "monospace", fontSize: "0.8rem" }}>
                                {String(v)}
                              </TableCell>
                            ))}
                          </TableRow>
                        );
                      })}
                    </TableBody>
                  </Table>
                </TableContainer>
              )
            ) : (
              <Box
                component="pre"
                sx={{
                  fontFamily: "monospace",
                  fontSize: "0.78rem",
                  p: 1.5,
                  borderRadius: 1,
                  bgcolor: "action.hover",
                  overflow: "auto",
                  maxHeight: 320,
                  whiteSpace: "pre-wrap",
                }}
              >
                {JSON.stringify(snapshots, null, 2)}
              </Box>
            )
          )}

          {snapshots === null && !loading && !error && (
            <Typography color="text.secondary" variant="body2">Select a volume and refresh to list snapshots.</Typography>
          )}
        </Stack>
      </DialogContent>
      <DialogActions>
        <IconButton size="small" onClick={reload} disabled={loading} sx={{ mr: "auto" }}>
          <RefreshIcon fontSize="small" />
        </IconButton>
        <Button onClick={onClose}>Close</Button>
      </DialogActions>
    </Dialog>
  );
}

// ── Create strategy dialog ────────────────────────────────────────────────────

function CreateStrategyDialog({
  backupApps,
  siteVols,
  exportedVols,
  onClose,
  onSuccess,
}: {
  backupApps: BackupApp[];
  siteVols: SiteVolume[];
  exportedVols: ExportedVolume[];
  onClose: () => void;
  onSuccess: () => void;
}) {
  const [name, setName] = useState("");
  const [via, setVia] = useState(backupApps[0]?.app ?? "");
  const [schedule, setSchedule] = useState<string>(BACKUP_SCHEDULES[2]);
  const [volumes, setVolumes] = useState<string[]>([]);

  const { execute, loading, error, clearError } = useOiAction();

  const opts = volumeOptions(siteVols, exportedVols);

  const handleClose = () => { clearError(); onClose(); };

  const handleSubmit = async () => {
    await execute("/backups/strategies/create", { name, via, schedule, volumes });
    onSuccess();
    handleClose();
  };

  const canSubmit = !!name && !!via && !!schedule && volumes.length > 0;

  return (
    <Dialog open onClose={handleClose} maxWidth="sm" fullWidth>
      <DialogTitle>New Backup Strategy</DialogTitle>
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
          <FormControl size="small">
            <InputLabel>Backup app</InputLabel>
            <Select label="Backup app" value={via} onChange={(e) => setVia(e.target.value)}>
              {backupApps.map((a) => (
                <MenuItem key={a.app} value={a.app} sx={{ fontFamily: "monospace" }}>
                  {a.app}
                </MenuItem>
              ))}
            </Select>
          </FormControl>
          <FormControl size="small">
            <InputLabel>Schedule</InputLabel>
            <Select label="Schedule" value={schedule} onChange={(e) => setSchedule(e.target.value)}>
              {BACKUP_SCHEDULES.map((s) => (
                <MenuItem key={s} value={s}>{s}</MenuItem>
              ))}
            </Select>
          </FormControl>
          <FormControl size="small">
            <InputLabel>Volumes</InputLabel>
            <Select
              multiple
              label="Volumes"
              value={volumes}
              onChange={(e) => setVolumes(typeof e.target.value === "string" ? [e.target.value] : e.target.value)}
              input={<OutlinedInput label="Volumes" />}
              renderValue={(selected) => (
                <Box sx={{ display: "flex", flexWrap: "wrap", gap: 0.5 }}>
                  {selected.map((v) => <Chip key={v} label={v} size="small" sx={{ fontFamily: "monospace" }} />)}
                </Box>
              )}
            >
              {opts.map((v) => (
                <MenuItem key={v} value={v} sx={{ fontFamily: "monospace" }}>{v}</MenuItem>
              ))}
            </Select>
          </FormControl>
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={handleClose} disabled={loading}>Cancel</Button>
        <Button variant="contained" onClick={() => void handleSubmit()} disabled={loading || !canSubmit}>
          {loading ? "Creating…" : "Create"}
        </Button>
      </DialogActions>
    </Dialog>
  );
}

// ── Register backup app dialog ────────────────────────────────────────────────

function RegisterBackupAppDialog({
  apps,
  onClose,
  onSuccess,
}: {
  apps: AppSummary[];
  onClose: () => void;
  onSuccess: () => void;
}) {
  const [app, setApp] = useState(apps[0]?.name ?? "");

  const { execute, loading, error, clearError } = useOiAction();

  const handleClose = () => { clearError(); onClose(); };

  const handleSubmit = async () => {
    await execute("/backups/apps/register", { app });
    onSuccess();
    handleClose();
  };

  return (
    <Dialog open onClose={handleClose} maxWidth="xs" fullWidth>
      <DialogTitle>Register Backup App</DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 0.5 }}>
          {error && <OiErrorAlert error={error} />}
          <FormControl size="small">
            <InputLabel>App</InputLabel>
            <Select
              label="App"
              value={app}
              onChange={(e) => setApp(e.target.value)}
              sx={{ fontFamily: "monospace" }}
            >
              {apps.map((a) => (
                <MenuItem key={a.name} value={a.name} sx={{ fontFamily: "monospace" }}>{a.name}</MenuItem>
              ))}
            </Select>
          </FormControl>
          <Typography variant="caption" color="text.secondary">
            The app's BSL script must declare save-snapshot, list-snapshots,
            and restore-snapshot actions.
          </Typography>
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={handleClose} disabled={loading}>Cancel</Button>
        <Button variant="contained" onClick={() => void handleSubmit()} disabled={loading || !app}>
          {loading ? "Registering…" : "Register"}
        </Button>
      </DialogActions>
    </Dialog>
  );
}

// ── Main page ─────────────────────────────────────────────────────────────────

// w[impl routes.backups]
export default function Backups() {
  const { data: strategies, loading: stratLoading, error: stratError, refetch: refetchStrat } =
    useOiQuery<BackupStrategy[]>("/backups/strategies/list", {});
  const { data: backupApps, loading: appsLoading, error: appsError, refetch: refetchApps } =
    useOiQuery<BackupApp[]>("/backups/apps/list", {});
  const { data: siteVols } = useOiQuery<SiteVolume[]>("/volumes/site/list", {});
  const { data: exportedVols } = useOiQuery<ExportedVolume[]>("/volumes/exported/list", {});
  const { data: allApps } = useOiQuery<AppSummary[]>("/apps/list", {});

  const { execute: doRun, loading: running } = useOiAction();
  const { execute: doDelete } = useOiAction();
  const { execute: doDeregister } = useOiAction();

  const [createStratOpen, setCreateStratOpen] = useState(false);
  const [registerAppOpen, setRegisterAppOpen] = useState(false);
  const [snapshotsFor, setSnapshotsFor] = useState<BackupStrategy | null>(null);
  const [runResults, setRunResults] = useState<{ strategy: string; results: BackupRunResult[] } | null>(null);

  const refreshAll = () => { refetchStrat(); refetchApps(); };

  const handleRun = async (strategyName: string) => {
    const res = await doRun("/backups/run", { strategy: strategyName }) as BackupRunResult[] | null;
    if (res) setRunResults({ strategy: strategyName, results: res });
  };

  const handleDeleteStrategy = async (name: string) => {
    await doDelete("/backups/strategies/delete", { name });
    refetchStrat();
  };

  const handleDeregisterApp = async (app: string) => {
    await doDeregister("/backups/apps/deregister", { app });
    refetchApps();
  };

  const anyLoading = stratLoading || appsLoading;

  return (
    <Box sx={{ p: 3, maxWidth: 900, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>Backups</Typography>
        <Tooltip title="Refresh">
          <span>
            <IconButton onClick={refreshAll} disabled={anyLoading} size="small">
              <RefreshIcon />
            </IconButton>
          </span>
        </Tooltip>
      </Box>

      {runResults && (
        <Alert severity="success" onClose={() => setRunResults(null)} sx={{ mb: 2 }}>
          Backup triggered for <strong>{runResults.strategy}</strong>.{" "}
          Operations: {runResults.results.map((r) => (
            <Box key={r.volume} component="span" sx={{ fontFamily: "monospace", mr: 1 }}>
              {r.volume} → {r.operation_id}
            </Box>
          ))}
        </Alert>
      )}

      <Stack spacing={4}>
        {/* Strategies */}
        <Box>
          <Box sx={{ display: "flex", alignItems: "center", mb: 1, gap: 1 }}>
            <Typography variant="subtitle1" sx={{ fontWeight: 600, flexGrow: 1 }}>Strategies</Typography>
            <Button
              size="small"
              startIcon={<AddIcon />}
              onClick={() => setCreateStratOpen(true)}
              disabled={!backupApps || backupApps.length === 0}
            >
              New
            </Button>
          </Box>

          {stratError && <OiErrorAlert error={stratError} />}
          {stratLoading && !strategies && <CircularProgress size={20} />}

          {!stratLoading && !stratError && strategies?.length === 0 && (
            <Typography color="text.secondary" variant="body2">
              No backup strategies.{" "}
              {backupApps?.length === 0
                ? "Register a backup app first."
                : "Create a strategy to get started."}
            </Typography>
          )}

          {strategies && strategies.length > 0 && (
            <TableContainer component={Paper} variant="outlined">
              <Table size="small">
                <TableHead>
                  <TableRow>
                    <TableCell>Name</TableCell>
                    <TableCell>Via</TableCell>
                    <TableCell>Schedule</TableCell>
                    <TableCell>Volumes</TableCell>
                    <TableCell width={100} />
                  </TableRow>
                </TableHead>
                <TableBody>
                  {strategies.map((s) => (
                    <TableRow key={s.name}>
                      <TableCell sx={{ fontFamily: "monospace", fontWeight: 500 }}>{s.name}</TableCell>
                      <TableCell sx={{ fontFamily: "monospace" }}>{s.via}</TableCell>
                      <TableCell>{s.schedule}</TableCell>
                      <TableCell>
                        <Box sx={{ display: "flex", flexWrap: "wrap", gap: 0.5 }}>
                          {s.volumes.map((v) => (
                            <Chip key={v} label={v} size="small" variant="outlined" sx={{ fontFamily: "monospace", fontSize: "0.7rem" }} />
                          ))}
                        </Box>
                      </TableCell>
                      <TableCell align="right" sx={{ px: 0.5, whiteSpace: "nowrap" }}>
                        <Tooltip title="List snapshots / restore">
                          <IconButton size="small" onClick={() => setSnapshotsFor(s)}>
                            <HistoryIcon sx={{ fontSize: 16 }} />
                          </IconButton>
                        </Tooltip>
                        <Tooltip title="Run backup now">
                          <span>
                            <IconButton size="small" onClick={() => void handleRun(s.name)} disabled={running}>
                              <PlayArrowIcon sx={{ fontSize: 16 }} />
                            </IconButton>
                          </span>
                        </Tooltip>
                        <Tooltip title="Delete strategy">
                          <IconButton size="small" onClick={() => void handleDeleteStrategy(s.name)}>
                            <DeleteOutlineIcon sx={{ fontSize: 16 }} />
                          </IconButton>
                        </Tooltip>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </TableContainer>
          )}
        </Box>

        <Divider />

        {/* Backup Apps */}
        <Box>
          <Box sx={{ display: "flex", alignItems: "center", mb: 1, gap: 1 }}>
            <Typography variant="subtitle1" sx={{ fontWeight: 600, flexGrow: 1 }}>Backup Apps</Typography>
            <Button size="small" startIcon={<AddIcon />} onClick={() => setRegisterAppOpen(true)}>
              Register
            </Button>
          </Box>

          {appsError && <OiErrorAlert error={appsError} />}
          {appsLoading && !backupApps && <CircularProgress size={20} />}

          {backupApps?.length === 0 && (
            <Typography color="text.secondary" variant="body2">
              No backup apps registered. Register a Seedling app that implements{" "}
              <Box component="span" sx={{ fontFamily: "monospace" }}>save-snapshot</Box>,{" "}
              <Box component="span" sx={{ fontFamily: "monospace" }}>list-snapshots</Box>, and{" "}
              <Box component="span" sx={{ fontFamily: "monospace" }}>restore-snapshot</Box> actions.
            </Typography>
          )}

          {backupApps && backupApps.length > 0 && (
            <TableContainer component={Paper} variant="outlined">
              <Table size="small">
                <TableHead>
                  <TableRow>
                    <TableCell>App</TableCell>
                    <TableCell width={40} />
                  </TableRow>
                </TableHead>
                <TableBody>
                  {backupApps.map((a) => (
                    <TableRow key={a.app}>
                      <TableCell sx={{ fontFamily: "monospace", fontWeight: 500 }}>
                        <Link to={`/apps/${a.app}`}>{a.app}</Link>
                      </TableCell>
                      <TableCell align="right" sx={{ px: 0.5 }}>
                        <Tooltip title="Deregister">
                          <IconButton size="small" onClick={() => void handleDeregisterApp(a.app)}>
                            <DeleteOutlineIcon sx={{ fontSize: 16 }} />
                          </IconButton>
                        </Tooltip>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </TableContainer>
          )}
        </Box>
      </Stack>

      {/* Dialogs */}
      {createStratOpen && backupApps && (
        <CreateStrategyDialog
          backupApps={backupApps}
          siteVols={siteVols ?? []}
          exportedVols={exportedVols ?? []}
          onClose={() => setCreateStratOpen(false)}
          onSuccess={() => { refetchStrat(); setCreateStratOpen(false); }}
        />
      )}

      {registerAppOpen && (
        <RegisterBackupAppDialog
          apps={allApps ?? []}
          onClose={() => setRegisterAppOpen(false)}
          onSuccess={() => { refetchApps(); setRegisterAppOpen(false); }}
        />
      )}

      {snapshotsFor && (
        <SnapshotsDialog
          key={snapshotsFor.name}
          strategy={snapshotsFor}
          onClose={() => setSnapshotsFor(null)}
        />
      )}
    </Box>
  );
}
