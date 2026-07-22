import LinkOffIcon from "@mui/icons-material/LinkOff";
import RefreshIcon from "@mui/icons-material/Refresh";
import {
  Box,
  Button,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Paper,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableRow,
  TextField,
  Typography,
} from "@mui/material";
import { useState } from "react";
import {
  IconActionButton,
  SolidActionButton,
} from "../components/ActionButton";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useOiQuery } from "../hooks/useOi";
import { useOiAction } from "../hooks/useOiAction";
import type { CanopyStatus } from "../lib/types";

// w[impl routes.canopy]
export default function Canopy() {
  const { data, loading, error, refetch } =
    useOiQuery<CanopyStatus>("/canopy/status", {});
  const { execute, loading: mutating, error: mutateError, clearError } =
    useOiAction();

  const [ticket, setTicket] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [confirmingDeregister, setConfirmingDeregister] = useState(false);

  const submitEnrol = async () => {
    const result = await execute("/canopy/enrol", {
      ticket: ticket.trim(),
      passphrase,
    });
    // Neither the ticket nor the passphrase is retained client-side after
    // submission — clear both whether or not the enrolment succeeded.
    setTicket("");
    setPassphrase("");
    if (result === null) return;
    refetch();
  };

  const submitDeregister = async () => {
    if ((await execute("/canopy/deregister", {})) === null) return;
    setConfirmingDeregister(false);
    refetch();
  };

  return (
    <Box sx={{ p: 3, maxWidth: 900, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          Canopy
        </Typography>
        <IconActionButton
          safety="read"
          tooltip="Refresh"
          onClick={refetch}
          disabled={loading}
        >
          <RefreshIcon />
        </IconActionButton>
        {data?.enrolled && (
          <SolidActionButton
            safety="dangerous"
            size="small"
            startIcon={<LinkOffIcon />}
            onClick={() => {
              clearError();
              setConfirmingDeregister(true);
            }}
          >
            Deregister
          </SolidActionButton>
        )}
      </Box>
      <Typography variant="body2" sx={{ color: "text.secondary", mb: 2 }}>
        Canopy is a fleet monitoring service. An enrolled seedling reports its
        status to the Canopy server it is registered with.
      </Typography>
      {error && <OiErrorAlert error={error} />}
      {loading && !data && (
        <Box sx={{ display: "flex", justifyContent: "center", mt: 4 }}>
          <CircularProgress />
        </Box>
      )}
      {data && !data.enrolled && (
        <Stack spacing={2}>
          <Typography>
            This seedling is not enrolled with Canopy. Paste an enrolment
            ticket issued by your Canopy server, together with its passphrase,
            to begin reporting.
          </Typography>
          <TextField
            label="Enrolment ticket"
            placeholder="Paste the enrolment ticket here"
            fullWidth
            multiline
            minRows={4}
            value={ticket}
            onChange={(e) => setTicket(e.target.value)}
            slotProps={{
              htmlInput: { style: { fontFamily: "monospace", fontSize: "0.8rem" } },
            }}
          />
          <TextField
            label="Passphrase"
            type="password"
            fullWidth
            value={passphrase}
            onChange={(e) => setPassphrase(e.target.value)}
            helperText="The ticket and passphrase are sent once and not retained in the browser."
          />
          {mutateError && <OiErrorAlert error={mutateError} />}
          <Box>
            <SolidActionButton
              safety="dangerous"
              onClick={submitEnrol}
              disabled={ticket.trim() === "" || passphrase === "" || mutating}
            >
              Enrol
            </SolidActionButton>
          </Box>
        </Stack>
      )}
      {data?.enrolled && (
        <Stack spacing={2}>
          <TableContainer component={Paper} variant="outlined">
            <Table size="small">
              <TableBody>
                <TableRow>
                  <TableCell sx={{ width: 180 }}>Server ID</TableCell>
                  <TableCell sx={{ fontFamily: "monospace", fontSize: "0.8rem" }}>
                    {data.server_id}
                  </TableCell>
                </TableRow>
                <TableRow>
                  <TableCell>Device ID</TableCell>
                  <TableCell sx={{ fontFamily: "monospace", fontSize: "0.8rem" }}>
                    {data.device_id}
                  </TableCell>
                </TableRow>
                <TableRow>
                  <TableCell>API URL</TableCell>
                  <TableCell sx={{ fontFamily: "monospace", fontSize: "0.8rem" }}>
                    {data.api_url}
                  </TableCell>
                </TableRow>
                <TableRow>
                  <TableCell>Last report</TableCell>
                  <TableCell>
                    {data.last_push_at
                      ? new Date(data.last_push_at).toLocaleString()
                      : "No reports attempted yet"}
                  </TableCell>
                </TableRow>
                {data.last_push_error && (
                  <TableRow>
                    <TableCell>Last report error</TableCell>
                    <TableCell sx={{ color: "error.main" }}>
                      {data.last_push_error}
                    </TableCell>
                  </TableRow>
                )}
              </TableBody>
            </Table>
          </TableContainer>
          {data.last_response !== undefined && (
            <details>
              <summary style={{ cursor: "pointer", userSelect: "none" }}>
                Last response from Canopy
              </summary>
              <Box
                component="pre"
                sx={{
                  m: "4px 0 0",
                  p: 1,
                  fontSize: "0.75rem",
                  overflowX: "auto",
                  bgcolor: "action.hover",
                  borderRadius: 1,
                }}
              >
                {JSON.stringify(data.last_response, null, 2)}
              </Box>
            </details>
          )}
          {mutateError && !confirmingDeregister && <OiErrorAlert error={mutateError} />}
        </Stack>
      )}
      <Dialog
        open={confirmingDeregister}
        onClose={() => setConfirmingDeregister(false)}
        fullWidth
        maxWidth="sm"
      >
        <DialogTitle>Deregister from Canopy</DialogTitle>
        <DialogContent>
          <Stack spacing={2} sx={{ mt: 1 }}>
            <Typography>
              Deregister this seedling from Canopy? It will stop reporting and
              its device credentials will be discarded. Re-enrolling requires a
              new enrolment ticket.
            </Typography>
            {mutateError && <OiErrorAlert error={mutateError} />}
          </Stack>
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setConfirmingDeregister(false)} disabled={mutating}>
            Cancel
          </Button>
          <SolidActionButton
            safety="dangerous"
            onClick={submitDeregister}
            disabled={mutating}
          >
            Deregister
          </SolidActionButton>
        </DialogActions>
      </Dialog>
    </Box>
  );
}
