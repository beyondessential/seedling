import RefreshIcon from "@mui/icons-material/Refresh";
import {
  Box,
  Chip,
  CircularProgress,
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
import { Link } from "react-router-dom";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useOiQuery } from "../hooks/useOi";
import type { ConnectedClients } from "../lib/types";

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

export default function Sessions() {
  const { data, loading, error, refetch } =
    useOiQuery<ConnectedClients>("/connected-clients/list", {});

  const webCount = data?.web.length ?? 0;
  const shellCount = data?.shells.length ?? 0;
  const forwardCount = data?.forwards.length ?? 0;
  const total = webCount + shellCount + forwardCount;

  return (
    <Box sx={{ p: 3, maxWidth: 900, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          Connected Clients
        </Typography>
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
      {data && total === 0 && (
        <Typography sx={{
          color: "text.secondary"
        }}>No active clients.</Typography>
      )}
      {data && total > 0 && (
        <Stack spacing={3}>
          {webCount > 0 && (
            <Box>
              <Typography variant="subtitle1" sx={{ mb: 1, fontWeight: 600 }}>
                Web UI ({webCount})
              </Typography>
              <TableContainer component={Paper} variant="outlined">
                <Table size="small">
                  <TableHead>
                    <TableRow>
                      <TableCell>ID</TableCell>
                      <TableCell>User</TableCell>
                      <TableCell>Connected</TableCell>
                      <TableCell>Last seen</TableCell>
                    </TableRow>
                  </TableHead>
                  <TableBody>
                    {/* w[impl routes.sessions] */}
                    {data.web.map((s) => (
                      <TableRow key={s.id}>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          {s.id.slice(0, 8)}
                        </TableCell>
                        <TableCell sx={{ fontFamily: "monospace" }}>
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

          {shellCount > 0 && (
            <Box>
              <Typography variant="subtitle1" sx={{ mb: 1, fontWeight: 600 }}>
                Shells ({shellCount})
              </Typography>
              <TableContainer component={Paper} variant="outlined">
                <Table size="small">
                  <TableHead>
                    <TableRow>
                      <TableCell>ID</TableCell>
                      <TableCell>App</TableCell>
                      <TableCell>Shell</TableCell>
                      <TableCell>Opened</TableCell>
                    </TableRow>
                  </TableHead>
                  <TableBody>
                    {data.shells.map((s) => (
                      <TableRow key={s.session_id}>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          {s.session_id.slice(0, 8)}
                        </TableCell>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          <Link to={`/apps/${s.app}`}>{s.app}</Link>
                        </TableCell>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          {s.name}
                        </TableCell>
                        <TableCell sx={{ color: "text.secondary" }}>
                          {new Date(s.opened_at).toLocaleString()}
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </TableContainer>
            </Box>
          )}

          {forwardCount > 0 && (
            <Box>
              <Typography variant="subtitle1" sx={{ mb: 1, fontWeight: 600 }}>
                Port Forwards ({forwardCount})
              </Typography>
              <TableContainer component={Paper} variant="outlined">
                <Table size="small">
                  <TableHead>
                    <TableRow>
                      <TableCell>ID</TableCell>
                      <TableCell>App</TableCell>
                      <TableCell>Service</TableCell>
                      <TableCell>Port</TableCell>
                      <TableCell>Proto</TableCell>
                      <TableCell>Opened</TableCell>
                    </TableRow>
                  </TableHead>
                  <TableBody>
                    {data.forwards.map((f) => (
                      <TableRow key={f.forward_id}>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          {f.forward_id.slice(0, 8)}
                        </TableCell>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          <Link to={`/apps/${f.app}`}>{f.app}</Link>
                        </TableCell>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          {f.service}
                        </TableCell>
                        <TableCell sx={{ fontFamily: "monospace" }}>
                          {f.port}
                        </TableCell>
                        <TableCell>
                          <Chip label={f.proto} size="small" variant="outlined" />
                        </TableCell>
                        <TableCell sx={{ color: "text.secondary" }}>
                          {new Date(f.opened_at).toLocaleString()}
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </TableContainer>
            </Box>
          )}
        </Stack>
      )}
    </Box>
  );
}
