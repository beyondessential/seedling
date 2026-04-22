import AddIcon from "@mui/icons-material/Add";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutlineOutlined";
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
import { useGuard } from "../components/SafetyModeProvider";
import { useOiQuery } from "../hooks/useOi";
import { useOiAction } from "../hooks/useOiAction";
import type { AuthorizedKey } from "../lib/types";

// w[impl routes.keys]
export default function Keys() {
  const { data, loading, error, refetch } =
    useOiQuery<AuthorizedKey[]>("/keys/list", {});
  const { execute, loading: mutating, error: mutateError, clearError } =
    useOiAction();
  const dangerGuard = useGuard("dangerous");

  const [dialogOpen, setDialogOpen] = useState(false);
  const [fingerprint, setFingerprint] = useState("");
  const [label, setLabel] = useState("");
  const [revoking, setRevoking] = useState<AuthorizedKey | null>(null);

  const fingerprintRe = /^[0-9a-f]{64}$/;
  const fingerprintValid = fingerprintRe.test(fingerprint.trim());

  const openAdd = () => {
    setFingerprint("");
    setLabel("");
    clearError();
    setDialogOpen(true);
  };

  const submitAdd = async () => {
    try {
      await execute("/keys/authorise", {
        fingerprint: fingerprint.trim(),
        label: label.trim() || "unnamed",
      });
      setDialogOpen(false);
      refetch();
    } catch {
      // error surfaced via mutateError
    }
  };

  const submitRevoke = async () => {
    if (!revoking) return;
    try {
      await execute("/keys/revoke", { fingerprint: revoking.fingerprint });
      setRevoking(null);
      refetch();
    } catch {
      // error surfaced via mutateError
    }
  };

  return (
    <Box sx={{ p: 3, maxWidth: 1100, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          Authorised OI Keys
        </Typography>
        <Tooltip title="Refresh">
          <span>
            <IconButton onClick={refetch} disabled={loading} size="small">
              <RefreshIcon />
            </IconButton>
          </span>
        </Tooltip>
        <Tooltip title={dangerGuard.reason ?? ""}>
          <span>
            <Button
              variant="contained"
              size="small"
              startIcon={<AddIcon />}
              onClick={openAdd}
              disabled={!dangerGuard.allowed}
            >
              Authorise key
            </Button>
          </span>
        </Tooltip>
      </Box>
      <Typography
        variant="body2"
        sx={{
          color: "text.secondary",
          mb: 2
        }}>
        Clients identify themselves with a 32-byte SPKI fingerprint of an
        Ed25519 raw public key. Only fingerprints listed here may open OI
        connections.
      </Typography>
      {error && <OiErrorAlert error={error} />}
      {loading && !data && (
        <Box sx={{ display: "flex", justifyContent: "center", mt: 4 }}>
          <CircularProgress />
        </Box>
      )}
      {data && data.length === 0 && (
        <Alert severity="warning">
          No authorised keys. Until at least one key is authorised, OI clients
          cannot connect.
        </Alert>
      )}
      {data && data.length > 0 && (
        <TableContainer component={Paper} variant="outlined">
          <Table size="small">
            <TableHead>
              <TableRow>
                <TableCell>Label</TableCell>
                <TableCell>Fingerprint (sha256)</TableCell>
                <TableCell>Added</TableCell>
                <TableCell align="right">Actions</TableCell>
              </TableRow>
            </TableHead>
            <TableBody>
              {data.map((k) => (
                <TableRow key={k.fingerprint} hover>
                  <TableCell>{k.label}</TableCell>
                  <TableCell sx={{ fontFamily: "monospace", fontSize: "0.8rem" }}>
                    {k.fingerprint}
                  </TableCell>
                  <TableCell>
                    {k.added_at ? new Date(k.added_at).toLocaleString() : "—"}
                  </TableCell>
                  <TableCell align="right">
                    <Tooltip title={dangerGuard.reason ?? "Revoke"}>
                      <span>
                        <IconButton
                          size="small"
                          onClick={() => {
                            clearError();
                            setRevoking(k);
                          }}
                          disabled={!dangerGuard.allowed}
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
      <Dialog open={dialogOpen} onClose={() => setDialogOpen(false)} fullWidth maxWidth="sm">
        <DialogTitle>Authorise OI key</DialogTitle>
        <DialogContent>
          <Stack spacing={2} sx={{ mt: 1 }}>
            <TextField
              label="Fingerprint"
              placeholder="64 lowercase hex characters"
              fullWidth
              value={fingerprint}
              onChange={(e) => setFingerprint(e.target.value)}
              error={fingerprint.length > 0 && !fingerprintValid}
              helperText={
                fingerprint.length > 0 && !fingerprintValid
                  ? "Expected 64 lowercase hex characters (sha256)"
                  : "Run `seedling-ctl probe` on the client to read its fingerprint"
              }
              slotProps={{ htmlInput: { style: { fontFamily: "monospace" } } }}
            />
            <TextField
              label="Label"
              placeholder="e.g. felix-laptop, ci-runner"
              fullWidth
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              helperText="Free-text description shown in lists; defaults to 'unnamed'."
            />
            {mutateError && <OiErrorAlert error={mutateError} />}
          </Stack>
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setDialogOpen(false)} disabled={mutating}>
            Cancel
          </Button>
          <Tooltip title={dangerGuard.reason ?? ""}>
            <span>
              <Button
                onClick={submitAdd}
                variant="contained"
                disabled={!fingerprintValid || mutating || !dangerGuard.allowed}
              >
                Authorise
              </Button>
            </span>
          </Tooltip>
        </DialogActions>
      </Dialog>
      <Dialog open={revoking !== null} onClose={() => setRevoking(null)} fullWidth maxWidth="sm">
        <DialogTitle>Revoke OI key</DialogTitle>
        <DialogContent>
          {revoking && (
            <Stack spacing={2} sx={{ mt: 1 }}>
              <Typography>
                Revoke <strong>{revoking.label}</strong>? Any active
                connections from this key remain open until they next reconnect.
              </Typography>
              <Box sx={{ fontFamily: "monospace", fontSize: "0.8rem", color: "text.secondary" }}>
                {revoking.fingerprint}
              </Box>
              {mutateError && <OiErrorAlert error={mutateError} />}
            </Stack>
          )}
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setRevoking(null)} disabled={mutating}>
            Cancel
          </Button>
          <Tooltip title={dangerGuard.reason ?? ""}>
            <span>
              <Button
                onClick={submitRevoke}
                variant="contained"
                color="error"
                disabled={mutating || !dangerGuard.allowed}
              >
                Revoke
              </Button>
            </span>
          </Tooltip>
        </DialogActions>
      </Dialog>
    </Box>
  );
}
