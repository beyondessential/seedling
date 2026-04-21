import AddIcon from "@mui/icons-material/Add";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutline";
import RefreshIcon from "@mui/icons-material/Refresh";
import {
  Alert,
  Box,
  Button,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  IconButton,
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
import { useState } from "react";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useOiQuery } from "../hooks/useOi";
import { useOiAction } from "../hooks/useOiAction";

interface RegistriesResponse {
  registries: string[];
}

// Hostname-with-optional-port. Allows alphanumerics, dots, hyphens, and an
// optional :NNN suffix.
const REGISTRY_RE = /^[a-zA-Z0-9]([a-zA-Z0-9-.]*[a-zA-Z0-9])?(:\d{1,5})?$/;

// w[impl routes.registries]
export default function Registries() {
  const { data, loading, error, refetch } =
    useOiQuery<RegistriesResponse>("/registries/list", {});
  const { execute, loading: mutating, error: mutateError, clearError } =
    useOiAction();

  const [dialogOpen, setDialogOpen] = useState(false);
  const [registry, setRegistry] = useState("");
  const [removing, setRemoving] = useState<string | null>(null);

  const registries = data?.registries ?? [];
  const trimmed = registry.trim();
  const registryValid = trimmed.length > 0 && REGISTRY_RE.test(trimmed);

  const openAdd = () => {
    setRegistry("");
    clearError();
    setDialogOpen(true);
  };

  const submitAdd = async () => {
    try {
      await execute("/registries/add", { registry: trimmed });
      setDialogOpen(false);
      refetch();
    } catch {
      // surfaced via mutateError
    }
  };

  const submitRemove = async () => {
    if (!removing) return;
    try {
      await execute("/registries/remove", { registry: removing });
      setRemoving(null);
      refetch();
    } catch {
      // surfaced via mutateError
    }
  };

  return (
    <Box sx={{ p: 3, maxWidth: 900, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          Container Registry Allowlist
        </Typography>
        <Tooltip title="Refresh">
          <span>
            <IconButton onClick={refetch} disabled={loading} size="small">
              <RefreshIcon />
            </IconButton>
          </span>
        </Tooltip>
        <Button
          variant="contained"
          size="small"
          startIcon={<AddIcon />}
          onClick={openAdd}
        >
          Add registry
        </Button>
      </Box>

      <Typography variant="body2" color="text.secondary" sx={{ mb: 2 }}>
        Apps may only pull container images from registries listed here.
        Removing a registry files a <code>disallowed_registry</code> fault on
        any app whose images reference it.
      </Typography>

      {error && <OiErrorAlert error={error} />}

      {loading && !data && (
        <Box sx={{ display: "flex", justifyContent: "center", mt: 4 }}>
          <CircularProgress />
        </Box>
      )}

      {data && registries.length === 0 && (
        <Alert severity="warning">
          The allowlist is empty. No container images can be pulled until at
          least one registry is added.
        </Alert>
      )}

      {registries.length > 0 && (
        <TableContainer component={Paper} variant="outlined">
          <Table size="small">
            <TableHead>
              <TableRow>
                <TableCell>Registry</TableCell>
                <TableCell align="right">Actions</TableCell>
              </TableRow>
            </TableHead>
            <TableBody>
              {registries.map((r) => (
                <TableRow key={r} hover>
                  <TableCell sx={{ fontFamily: "monospace" }}>{r}</TableCell>
                  <TableCell align="right">
                    <Tooltip title="Remove">
                      <IconButton
                        size="small"
                        onClick={() => {
                          clearError();
                          setRemoving(r);
                        }}
                      >
                        <DeleteOutlineIcon fontSize="small" />
                      </IconButton>
                    </Tooltip>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </TableContainer>
      )}

      <Dialog open={dialogOpen} onClose={() => setDialogOpen(false)} fullWidth maxWidth="sm">
        <DialogTitle>Add registry</DialogTitle>
        <DialogContent>
          <Stack spacing={2} sx={{ mt: 1 }}>
            <TextField
              label="Registry hostname"
              placeholder="docker.io"
              fullWidth
              value={registry}
              onChange={(e) => setRegistry(e.target.value)}
              error={trimmed.length > 0 && !registryValid}
              helperText={
                trimmed.length > 0 && !registryValid
                  ? "Hostname or hostname:port"
                  : "Examples: docker.io, ghcr.io, registry.example.com:5000"
              }
              slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
            />
            {mutateError && <OiErrorAlert error={mutateError} />}
          </Stack>
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setDialogOpen(false)} disabled={mutating}>
            Cancel
          </Button>
          <Button
            onClick={submitAdd}
            variant="contained"
            disabled={!registryValid || mutating}
          >
            Add
          </Button>
        </DialogActions>
      </Dialog>

      <Dialog open={removing !== null} onClose={() => setRemoving(null)} fullWidth maxWidth="sm">
        <DialogTitle>Remove registry</DialogTitle>
        <DialogContent>
          {removing && (
            <Stack spacing={2} sx={{ mt: 1 }}>
              <Typography>
                Remove <code>{removing}</code> from the allowlist? Apps with
                images on this registry will be flagged as faulted.
              </Typography>
              {mutateError && <OiErrorAlert error={mutateError} />}
            </Stack>
          )}
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setRemoving(null)} disabled={mutating}>
            Cancel
          </Button>
          <Button
            onClick={submitRemove}
            variant="contained"
            color="error"
            disabled={mutating}
          >
            Remove
          </Button>
        </DialogActions>
      </Dialog>
    </Box>
  );
}
