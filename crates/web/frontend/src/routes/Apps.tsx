import AddIcon from "@mui/icons-material/Add";
import RefreshIcon from "@mui/icons-material/Refresh";
import {
  Box,
  Button,
  Chip,
  CircularProgress,
  IconButton,
  Paper,
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
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useOiQuery } from "../hooks/useOi";
import { useEventRefresh } from "../hooks/useEventRefresh";
import { statusColor, statusLabel } from "../lib/status";
import type { AppSummary, SeedlingEvent } from "../lib/types";

const APP_LIST_EVENTS: Set<string> = new Set([
  "AppRegistered", "AppDeregistered", "AppUpdated",
  "OperationStarted", "OperationCompleted", "OperationFailed",
  "FaultFiled", "FaultCleared", "ResourceStopped", "ResourceUnstopped",
]);

export default function Apps() {
  const { data, loading, error, refetch } =
    useOiQuery<AppSummary[]>("/apps/list", {});
  const matchesApps = useCallback((ev: SeedlingEvent) => APP_LIST_EVENTS.has(ev.type), []);
  useEventRefresh(refetch, matchesApps);
  const navigate = useNavigate();

  return (
    <Box sx={{ p: 3, maxWidth: 900, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          Apps
        </Typography>
        <Button
          size="small"
          variant="contained"
          startIcon={<AddIcon />}
          component={Link}
          to="/apps/new"
        >
          New app
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
        <TableContainer component={Paper} variant="outlined">
          <Table size="small">
            <TableHead>
              <TableRow>
                <TableCell>Name</TableCell>
                <TableCell>Status</TableCell>
              </TableRow>
            </TableHead>
            <TableBody>
              {data.length === 0 && (
                <TableRow>
                  <TableCell colSpan={2} align="center" sx={{ color: "text.secondary", py: 4 }}>
                    No apps registered.
                  </TableCell>
                </TableRow>
              )}
              {data.map((app) => (
                <TableRow
                  key={app.name}
                  hover
                  onClick={() => void navigate(`/apps/${app.name}`)}
                  sx={{ cursor: "pointer" }}
                >
                  <TableCell sx={{ fontWeight: 500 }}>{app.name}</TableCell>
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
                    </Box>
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
