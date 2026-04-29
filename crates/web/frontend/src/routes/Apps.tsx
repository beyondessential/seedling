import AddIcon from "@mui/icons-material/Add";
import CancelIcon from "@mui/icons-material/Cancel";
import RefreshIcon from "@mui/icons-material/Refresh";
import StopIcon from "@mui/icons-material/Stop";
import {
  Box,
  Chip,
  CircularProgress,
  Divider,
  IconButton,
  Paper,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  Tooltip,
  Typography,
} from "@mui/material";
import { useCallback } from "react";
import { Link, useNavigate } from "react-router-dom";
import {
  IconActionButton,
  SolidActionButton,
} from "../components/ActionButton";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useOiAction } from "../hooks/useOiAction";
import { useOiQuery } from "../hooks/useOi";
import { useEventRefresh } from "../hooks/useEventRefresh";
import { statusColor, statusLabel } from "../lib/status";
import type { Actor, AppSummary, ConnectedClients, SeedlingEvent } from "../lib/types";

const APP_LIST_EVENTS: Set<string> = new Set([
  "AppRegistered", "AppDeregistered", "AppUpdated", "AppPhaseChanged",
  "OperationStarted", "OperationCompleted", "OperationFailed",
  "FaultFiled", "FaultCleared", "ResourceStopped", "ResourceUnstopped",
]);

const SESSION_EVENTS: Set<string> = new Set([
  "WebSessionStarted", "WebSessionStopped",
  "ShellStarted", "ShellExited",
  "ForwardStarted", "ForwardStopped",
]);

function actorLabel(actor?: Actor): string {
  if (!actor) return "—";
  return actor.display ?? actor.id ?? actor.kind ?? "—";
}

function formatRelative(ts: string): string {
  const delta = Date.now() - new Date(ts).getTime();
  if (delta < 0) return "in the future";
  const secs = Math.floor(delta / 1000);
  if (secs < 60) return `${secs}s ago`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

export default function Apps() {
  const { data: apps, loading: appsLoading, error: appsError, refetch: refetchApps } =
    useOiQuery<AppSummary[]>("/apps/list", {});
  const { data: clients, loading: clientsLoading, error: clientsError, refetch: refetchClients } =
    useOiQuery<ConnectedClients>("/connected-clients/list", {});
  const { execute: stopShell } = useOiAction();
  const { execute: stopForward } = useOiAction();
  const { execute: cancelOp, loading: cancelling } = useOiAction();

  const matchApps = useCallback((ev: SeedlingEvent) => APP_LIST_EVENTS.has(ev.type), []);
  const matchSessions = useCallback((ev: SeedlingEvent) => SESSION_EVENTS.has(ev.type), []);
  useEventRefresh(refetchApps, matchApps);
  useEventRefresh(refetchClients, matchSessions);

  const navigate = useNavigate();

  const handleStopShell = async (sessionId: string) => {
    await stopShell("/shells/stop", { session_id: sessionId });
    refetchClients();
  };

  const handleStopForward = async (forwardId: string) => {
    await stopForward("/forwards/stop", { forward_id: forwardId });
    refetchClients();
  };

  const handleCancelOp = async (appName: string) => {
    try {
      await cancelOp("/apps/action/cancel", { app: appName });
      refetchApps();
    } catch {
      // surfaced by useOiAction globally
    }
  };

  const webSessions = clients?.web ?? [];
  const shells = clients?.shells ?? [];
  const forwards = clients?.forwards ?? [];
  const actors = clients?.actors ?? [];

  return (
    <Box sx={{ p: 3, maxWidth: 900, mx: "auto" }}>
      {/* Apps */}
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>Apps</Typography>
        <SolidActionButton
          safety="write"
          size="small"
          startIcon={<AddIcon />}
          onClick={() => navigate("/apps/new")}
        >
          New app
        </SolidActionButton>
        <IconActionButton
          safety="read"
          tooltip="Refresh"
          onClick={refetchApps}
          disabled={appsLoading}
        >
          <RefreshIcon />
        </IconActionButton>
      </Box>
      {appsError && <OiErrorAlert error={appsError} />}
      {appsLoading && !apps && (
        <Box sx={{ display: "flex", justifyContent: "center", mt: 4 }}>
          <CircularProgress />
        </Box>
      )}
      {apps && (
        <TableContainer component={Paper} variant="outlined">
          <Table size="small">
            <TableHead>
              <TableRow>
                <TableCell>Name</TableCell>
                <TableCell>Status</TableCell>
              </TableRow>
            </TableHead>
            <TableBody>
              {apps.length === 0 && (
                <TableRow>
                  <TableCell colSpan={2} align="center" sx={{ color: "text.secondary", py: 4 }}>
                    No apps registered.
                  </TableCell>
                </TableRow>
              )}
              {apps.map((app) => (
                <TableRow
                  key={app.name}
                  hover
                  onClick={() => void navigate(`/apps/${app.name}`)}
                  sx={{ cursor: "pointer" }}
                >
                  <TableCell sx={{ fontWeight: 500 }}>
                    {app.name}
                    {app.description && (
                      <Typography
                        variant="caption"
                        component="div"
                        sx={{
                          color: "text.secondary",
                          fontWeight: 400,
                          mt: 0.25,
                        }}
                      >
                        {app.description}
                      </Typography>
                    )}
                  </TableCell>
                  <TableCell>
                    <Box sx={{ display: "flex", gap: 0.5, alignItems: "center" }}>
                      <Chip
                        label={statusLabel(app.status, app.action_name)}
                        color={statusColor(app.status)}
                        size="small"
                      />
                      {app.has_stopped_resources && (
                        <Chip label="partially running" size="small" color="warning" variant="outlined" />
                      )}
                      {(app.status === "installing" ||
                        app.status === "operating") && (
                        <IconActionButton
                          safety="dangerous"
                          tooltip="Cancel operation"
                          disabled={cancelling}
                          onClick={(e) => {
                            e.stopPropagation();
                            void handleCancelOp(app.name);
                          }}
                        >
                          <CancelIcon sx={{ fontSize: 16 }} />
                        </IconActionButton>
                      )}
                    </Box>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </TableContainer>
      )}
      {/* Sessions */}
      {(webSessions.length > 0 || shells.length > 0 || forwards.length > 0 || actors.length > 0) && (
        <>
          <Divider sx={{ my: 4 }} />
          <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
            <Typography variant="h5" sx={{ flexGrow: 1 }}>Active Sessions</Typography>
            <Tooltip title="Refresh">
              <span>
                <IconButton onClick={refetchClients} disabled={clientsLoading} size="small">
                  <RefreshIcon />
                </IconButton>
              </span>
            </Tooltip>
          </Box>
          {clientsError && <OiErrorAlert error={clientsError} />}
          <Stack spacing={3}>
            {/* w[impl routes.sessions] */}
            {webSessions.length > 0 && (
              <Box>
                <Typography variant="subtitle1" sx={{ mb: 1, fontWeight: 600 }}>
                  Web UI ({webSessions.length})
                </Typography>
                <TableContainer component={Paper} variant="outlined">
                  <Table size="small">
                    <TableHead>
                      <TableRow>
                        <TableCell>User</TableCell>
                        <TableCell>Connected</TableCell>
                        <TableCell>Last seen</TableCell>
                      </TableRow>
                    </TableHead>
                    <TableBody>
                      {webSessions.map((s) => (
                        <TableRow key={s.id}>
                          <TableCell sx={{ color: "text.secondary" }}>
                            {s.actor_display ?? s.actor_id ?? s.actor_kind ?? "—"}
                          </TableCell>
                          <TableCell sx={{ color: "text.secondary" }}>
                            {new Date(s.connected_at).toLocaleString()}
                          </TableCell>
                          <TableCell
                            sx={{ color: "text.secondary" }}
                            title={new Date(s.last_seen).toLocaleString()}
                          >
                            {formatRelative(s.last_seen)}
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </TableContainer>
              </Box>
            )}

            {/* w[impl sessions.actor-activity] */}
            {actors.length > 0 && (
              <Box>
                <Typography variant="subtitle1" sx={{ mb: 1, fontWeight: 600 }}>
                  Active operators ({actors.length})
                </Typography>
                <Typography
                  variant="caption"
                  sx={{ color: "text.secondary", display: "block", mb: 1 }}
                >
                  Operators with attributed activity in the last 10 minutes.
                  Includes web users and direct CLI users (seedling-ctl).
                </Typography>
                <TableContainer component={Paper} variant="outlined">
                  <Table size="small">
                    <TableHead>
                      <TableRow>
                        <TableCell>User</TableCell>
                        <TableCell>Via</TableCell>
                        <TableCell>Last action</TableCell>
                        <TableCell>Last seen</TableCell>
                      </TableRow>
                    </TableHead>
                    <TableBody>
                      {actors.map((a) => (
                        <TableRow key={`${a.actor_kind}:${a.actor_id}`}>
                          <TableCell sx={{ fontFamily: "monospace" }}>
                            {a.actor_display ?? a.actor_id}
                          </TableCell>
                          <TableCell>
                            <Chip label={a.actor_kind} size="small" variant="outlined" />
                          </TableCell>
                          <TableCell sx={{ color: "text.secondary" }}>
                            {a.last_action}
                          </TableCell>
                          <TableCell
                            sx={{ color: "text.secondary" }}
                            title={new Date(a.last_seen).toLocaleString()}
                          >
                            {formatRelative(a.last_seen)}
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </TableContainer>
              </Box>
            )}

            {shells.length > 0 && (
              <Box>
                <Typography variant="subtitle1" sx={{ mb: 1, fontWeight: 600 }}>
                  Shells ({shells.length})
                </Typography>
                <TableContainer component={Paper} variant="outlined">
                  <Table size="small">
                    <TableHead>
                      <TableRow>
                        <TableCell>App</TableCell>
                        <TableCell>Shell</TableCell>
                        <TableCell>Actor</TableCell>
                        <TableCell>Opened</TableCell>
                        <TableCell width={40} />
                      </TableRow>
                    </TableHead>
                    <TableBody>
                      {shells.map((s) => (
                        <TableRow key={s.session_id}>
                          <TableCell sx={{ fontFamily: "monospace" }}>
                            {s.app === "_volumes"
                              ? <Typography variant="caption" sx={{
                              color: "text.secondary"
                            }}>volumes</Typography>
                              : <Link to={`/apps/${s.app}`}>{s.app}</Link>}
                          </TableCell>
                          <TableCell sx={{ fontFamily: "monospace" }}>{s.name}</TableCell>
                          <TableCell sx={{ color: "text.secondary" }}>{actorLabel(s.actor)}</TableCell>
                          <TableCell sx={{ color: "text.secondary" }}>
                            {new Date(s.opened_at).toLocaleString()}
                          </TableCell>
                          <TableCell align="right" sx={{ px: 0.5 }}>
                            <IconActionButton
                              safety="dangerous"
                              tooltip="Stop shell"
                              onClick={() => void handleStopShell(s.session_id)}
                            >
                              <StopIcon sx={{ fontSize: 16 }} />
                            </IconActionButton>
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </TableContainer>
              </Box>
            )}

            {forwards.length > 0 && (
              <Box>
                <Typography variant="subtitle1" sx={{ mb: 1, fontWeight: 600 }}>
                  Port Forwards ({forwards.length})
                </Typography>
                <TableContainer component={Paper} variant="outlined">
                  <Table size="small">
                    <TableHead>
                      <TableRow>
                        <TableCell>App</TableCell>
                        <TableCell>Service</TableCell>
                        <TableCell>Port</TableCell>
                        <TableCell>Proto</TableCell>
                        <TableCell>Actor</TableCell>
                        <TableCell>Opened</TableCell>
                        <TableCell width={40} />
                      </TableRow>
                    </TableHead>
                    <TableBody>
                      {forwards.map((f) => (
                        <TableRow key={f.forward_id}>
                          <TableCell sx={{ fontFamily: "monospace" }}>
                            <Link to={`/apps/${f.app}`}>{f.app}</Link>
                          </TableCell>
                          <TableCell sx={{ fontFamily: "monospace" }}>{f.service}</TableCell>
                          <TableCell sx={{ fontFamily: "monospace" }}>{f.port}</TableCell>
                          <TableCell>
                            <Chip label={f.proto} size="small" variant="outlined" />
                          </TableCell>
                          <TableCell sx={{ color: "text.secondary" }}>{actorLabel(f.actor)}</TableCell>
                          <TableCell sx={{ color: "text.secondary" }}>
                            {new Date(f.opened_at).toLocaleString()}
                          </TableCell>
                          <TableCell align="right" sx={{ px: 0.5 }}>
                            <IconActionButton
                              safety="dangerous"
                              tooltip="Stop forward"
                              onClick={() => void handleStopForward(f.forward_id)}
                            >
                              <StopIcon sx={{ fontSize: 16 }} />
                            </IconActionButton>
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </TableContainer>
              </Box>
            )}
          </Stack>
        </>
      )}
    </Box>
  );
}
