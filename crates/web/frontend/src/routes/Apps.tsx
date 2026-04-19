import RefreshIcon from "@mui/icons-material/Refresh";
import {
  Alert,
  Box,
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
import { Link } from "react-router-dom";
import { useOiQuery } from "../hooks/useOi";
import { statusColor, statusLabel } from "../lib/status";
import type { AppSummary } from "../lib/types";

export default function Apps() {
  const { data, loading, error, refetch } =
    useOiQuery<AppSummary[]>("/apps/list", {});

  return (
    <Box sx={{ p: 3, maxWidth: 900, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          Apps
        </Typography>
        <Tooltip title="Refresh">
          <span>
            <IconButton onClick={refetch} disabled={loading} size="small">
              <RefreshIcon />
            </IconButton>
          </span>
        </Tooltip>
      </Box>

      {error && (
        <Alert severity="error" sx={{ mb: 2 }}>
          {error}
        </Alert>
      )}

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
                <TableRow key={app.name} hover>
                  <TableCell>
                    <Link
                      to={`/apps/${app.name}`}
                      style={{ textDecoration: "none", color: "inherit", fontWeight: 500 }}
                    >
                      {app.name}
                    </Link>
                  </TableCell>
                  <TableCell>
                    <Chip
                      label={statusLabel(app.status, app.action_name)}
                      color={statusColor(app.status)}
                      size="small"
                    />
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
