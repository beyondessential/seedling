import RefreshIcon from "@mui/icons-material/Refresh";
import {
  Alert,
  Box,
  Chip,
  CircularProgress,
  Paper,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableRow,
  Typography,
} from "@mui/material";
import {
  IconActionButton,
  SolidActionButton,
} from "../components/ActionButton";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useOiQuery } from "../hooks/useOi";
import { useOiAction } from "../hooks/useOiAction";

interface Offer {
  offer_id: string;
  agent: string;
  endpoint: string;
  via: string | null;
  offered_at: string;
}

interface LastReport {
  at: string;
  ok: boolean;
  error?: string;
}

interface CanopyStatus {
  enabled: boolean;
  offer: Offer | null;
  offers: number;
  server_id: string | null;
  last_report: LastReport | null;
}

// w[impl routes.canopy]
export default function Canopy() {
  const { data, loading, error, refetch } = useOiQuery<CanopyStatus>(
    "/canopy/status",
    {},
  );
  const {
    execute,
    loading: mutating,
    error: mutateError,
  } = useOiAction();

  const setEnabled = async (enabled: boolean) => {
    if ((await execute("/canopy/settings/set", { enabled })) === null) return;
    refetch();
  };

  const report = async () => {
    if ((await execute("/canopy/report", {})) === null) return;
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
      </Box>
      <Typography variant="body2" sx={{ color: "text.secondary", mb: 2 }}>
        Seedling has no Canopy identity of its own. A connected client — normally{" "}
        <code>bestool</code> on this host — offers to carry Seedling&apos;s Canopy
        requests and issues them under its own identity. Seedling reports its
        health through that channel; nothing happens on a host where no client
        offers one.
      </Typography>

      {error && <OiErrorAlert error={error} />}
      {mutateError && <OiErrorAlert error={mutateError} />}
      {loading && !data && (
        <Box sx={{ display: "flex", justifyContent: "center", mt: 4 }}>
          <CircularProgress />
        </Box>
      )}

      {data && (
        <Stack spacing={2}>
          <Paper variant="outlined" sx={{ p: 2 }}>
            <Box sx={{ display: "flex", alignItems: "center", gap: 2 }}>
              <Box sx={{ flexGrow: 1 }}>
                <Typography variant="subtitle1">Canopy access</Typography>
                <Typography variant="body2" sx={{ color: "text.secondary" }}>
                  {data.enabled
                    ? "Offers are accepted and health is reported."
                    : "Offers are refused. Turning this off also revokes any live offer immediately."}
                </Typography>
              </Box>
              <Chip
                size="small"
                label={data.enabled ? "Enabled" : "Disabled"}
                color={data.enabled ? "success" : "default"}
              />
              <SolidActionButton
                safety={data.enabled ? "dangerous" : "write"}
                size="small"
                disabled={mutating}
                onClick={() => setEnabled(!data.enabled)}
              >
                {data.enabled ? "Disable" : "Enable"}
              </SolidActionButton>
            </Box>
          </Paper>

          <Paper variant="outlined" sx={{ p: 2 }}>
            <Typography variant="subtitle1" sx={{ mb: 1 }}>
              Carrying client
            </Typography>
            {data.offer ? (
              <TableContainer>
                <Table size="small">
                  <TableBody>
                    <TableRow>
                      <TableCell sx={{ width: 140 }}>Agent</TableCell>
                      <TableCell sx={{ fontFamily: "monospace" }}>
                        {data.offer.agent}
                      </TableCell>
                    </TableRow>
                    <TableRow>
                      <TableCell>Endpoint</TableCell>
                      <TableCell sx={{ fontFamily: "monospace" }}>
                        {data.offer.endpoint}
                      </TableCell>
                    </TableRow>
                    {data.offer.via && (
                      <TableRow>
                        <TableCell>Via</TableCell>
                        <TableCell>{data.offer.via}</TableCell>
                      </TableRow>
                    )}
                    <TableRow>
                      <TableCell>Offered</TableCell>
                      <TableCell>{data.offer.offered_at}</TableCell>
                    </TableRow>
                    {data.offers > 1 && (
                      <TableRow>
                        <TableCell>Other offers</TableCell>
                        <TableCell>
                          {data.offers - 1} older offer
                          {data.offers - 1 === 1 ? "" : "s"} would take over if
                          this one ended
                        </TableCell>
                      </TableRow>
                    )}
                  </TableBody>
                </Table>
              </TableContainer>
            ) : (
              <Alert severity="info">
                No client is currently offering to reach Canopy. On a host
                without <code>bestool</code> running this is expected, and
                nothing is wrong.
              </Alert>
            )}
          </Paper>

          <Paper variant="outlined" sx={{ p: 2 }}>
            <Box sx={{ display: "flex", alignItems: "center", gap: 2, mb: 1 }}>
              <Typography variant="subtitle1" sx={{ flexGrow: 1 }}>
                Reporting
              </Typography>
              <SolidActionButton
                safety="write"
                size="small"
                disabled={mutating || !data.enabled || !data.offer}
                onClick={report}
              >
                Report now
              </SolidActionButton>
            </Box>
            <TableContainer>
              <Table size="small">
                <TableBody>
                  <TableRow>
                    <TableCell sx={{ width: 140 }}>Server</TableCell>
                    <TableCell sx={{ fontFamily: "monospace" }}>
                      {data.server_id ?? "not yet resolved"}
                    </TableCell>
                  </TableRow>
                  <TableRow>
                    <TableCell>Last report</TableCell>
                    <TableCell>
                      {data.last_report ? (
                        <Box
                          sx={{
                            display: "flex",
                            alignItems: "center",
                            gap: 1,
                            flexWrap: "wrap",
                          }}
                        >
                          <Chip
                            size="small"
                            label={data.last_report.ok ? "OK" : "Failed"}
                            color={data.last_report.ok ? "success" : "error"}
                          />
                          <span>{data.last_report.at}</span>
                        </Box>
                      ) : (
                        "none since this daemon started"
                      )}
                    </TableCell>
                  </TableRow>
                  {data.last_report?.error && (
                    <TableRow>
                      <TableCell>Error</TableCell>
                      <TableCell sx={{ fontFamily: "monospace" }}>
                        {data.last_report.error}
                      </TableCell>
                    </TableRow>
                  )}
                </TableBody>
              </Table>
            </TableContainer>
          </Paper>
        </Stack>
      )}
    </Box>
  );
}
