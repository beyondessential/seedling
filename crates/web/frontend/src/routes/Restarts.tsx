import RefreshIcon from "@mui/icons-material/Refresh";
import {
  Box,
  Chip,
  CircularProgress,
  MenuItem,
  Paper,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableRow,
  TextField,
  Typography,
} from "@mui/material";
import { useMemo, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import {
  IconActionButton,
  SolidActionButton,
} from "../components/ActionButton";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useOiQuery } from "../hooks/useOi";
import { useOiAction } from "../hooks/useOiAction";
import type { AppSummary, RestartRecord, RestartSettings } from "../lib/types";

/** How the previous run ended, in the terms an operator thinks in. */
function exitLabel(r: RestartRecord): string {
  if (r.exit_code === null || r.exit_code === undefined) return "unknown";
  switch (r.exit_kind) {
    case "signalled":
      return `signal ${r.exit_code}`;
    case "dumped":
      return `signal ${r.exit_code} (core dumped)`;
    default:
      return `exit ${r.exit_code}`;
  }
}

// w[impl routes.restarts]
export default function Restarts() {
  const [app, setApp] = useState("");
  // An instance filter only ever arrives by link — from the restart chip on an
  // app's resource table — so it lives in the URL rather than in a control.
  const [search, setSearch] = useSearchParams();
  const instance = search.get("instance") ?? "";
  const params = useMemo(
    () => ({
      ...(app ? { app } : {}),
      ...(instance ? { instance } : {}),
    }),
    [app, instance],
  );

  const { data, loading, error, refetch } = useOiQuery<RestartRecord[]>(
    "/restarts/list",
    params,
  );
  const { data: apps } = useOiQuery<AppSummary[]>("/apps/list", {});
  const {
    data: settings,
    error: settingsError,
    refetch: refetchSettings,
  } = useOiQuery<RestartSettings>("/restarts/settings/get", {});
  const {
    execute,
    loading: mutating,
    error: mutateError,
  } = useOiAction();

  const [threshold, setThreshold] = useState("");
  const [windowMins, setWindowMins] = useState("");

  const saveSettings = async () => {
    const body: Record<string, number> = {};
    if (threshold !== "") body.threshold = Number(threshold);
    if (windowMins !== "") body.window_secs = Number(windowMins) * 60;
    if (Object.keys(body).length === 0) return;
    if ((await execute("/restarts/settings/set", body)) === null) return;
    setThreshold("");
    setWindowMins("");
    refetchSettings();
  };

  return (
    <Box sx={{ p: 3, maxWidth: 1100, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          Restarts
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
        Every container restart Seedling observes or performs. Recovery from an
        unexpected exit counts towards the crash-loop rate; restarts Seedling
        performed deliberately — rolling updates, replacements — are recorded
        but do not, so a rollout never reads as a crash burst.
      </Typography>

      {settingsError && <OiErrorAlert error={settingsError} />}
      {mutateError && <OiErrorAlert error={mutateError} />}

      <Paper variant="outlined" sx={{ p: 2, mb: 3 }}>
        <Typography variant="subtitle1">Crash-loop rate</Typography>
        <Typography variant="body2" sx={{ color: "text.secondary", mb: 2 }}>
          {settings
            ? `A crash_loop fault is filed once an instance records ${settings.threshold} recovery restarts within ${settings.window_secs / 60} minutes.`
            : "Loading…"}
        </Typography>
        <Stack
          direction="row"
          spacing={2}
          sx={{ alignItems: "center", flexWrap: "wrap" }}
        >
          <TextField
            label="Threshold"
            size="small"
            type="number"
            value={threshold}
            onChange={(e) => setThreshold(e.target.value)}
            placeholder={settings ? String(settings.threshold) : ""}
            helperText="restarts (min 2)"
            sx={{ width: 160 }}
          />
          <TextField
            label="Window"
            size="small"
            type="number"
            value={windowMins}
            onChange={(e) => setWindowMins(e.target.value)}
            placeholder={settings ? String(settings.window_secs / 60) : ""}
            helperText="minutes (min 1)"
            sx={{ width: 160 }}
          />
          <SolidActionButton
            safety="write"
            size="small"
            disabled={mutating || (threshold === "" && windowMins === "")}
            onClick={saveSettings}
          >
            Save
          </SolidActionButton>
        </Stack>
      </Paper>

      <Stack
        direction="row"
        spacing={1}
        sx={{ alignItems: "center", mb: 2 }}
      >
        <TextField
          select
          label="App"
          size="small"
          value={app}
          onChange={(e) => setApp(e.target.value)}
          sx={{ minWidth: 220 }}
        >
          <MenuItem value="">All apps</MenuItem>
          {(apps ?? []).map((a) => (
            <MenuItem key={a.name} value={a.name}>
              {a.name}
            </MenuItem>
          ))}
        </TextField>
        {instance && (
          <Chip
            label={`instance ${instance.slice(0, 12)}`}
            size="small"
            onDelete={() => setSearch({})}
            sx={{ fontFamily: "monospace" }}
          />
        )}
      </Stack>

      {error && <OiErrorAlert error={error} />}
      {loading && !data && (
        <Box sx={{ display: "flex", justifyContent: "center", mt: 4 }}>
          <CircularProgress />
        </Box>
      )}
      {data && data.length === 0 && (
        <Typography sx={{ color: "text.secondary" }}>
          No restarts recorded.
        </Typography>
      )}
      {data && data.length > 0 && (
        <Table size="small">
          <TableHead>
            <TableRow>
              <TableCell>When</TableCell>
              <TableCell>App</TableCell>
              <TableCell>Resource</TableCell>
              <TableCell>Instance</TableCell>
              <TableCell>Cause</TableCell>
              <TableCell>Exit</TableCell>
              <TableCell align="right">Gen</TableCell>
            </TableRow>
          </TableHead>
          <TableBody>
            {data.map((r) => (
              <TableRow key={r.id}>
                <TableCell sx={{ whiteSpace: "nowrap" }}>
                  {new Date(r.timestamp).toLocaleString()}
                </TableCell>
                <TableCell>
                  <Link to={`/apps/${r.app}`} style={{ color: "inherit" }}>
                    {r.app}
                  </Link>
                </TableCell>
                <TableCell sx={{ fontFamily: "monospace" }}>
                  {r.resource_name
                    ? `${r.resource_type}/${r.resource_name}`
                    : (r.resource_type ?? "—")}
                </TableCell>
                <TableCell sx={{ fontFamily: "monospace" }}>
                  {r.instance_id.slice(0, 12)}
                </TableCell>
                <TableCell>
                  {/* The distinction is the whole point of recording the
                      cause: only recovery rows move the rate. */}
                  <Chip
                    size="small"
                    label={r.cause}
                    color={r.cause === "recovery" ? "warning" : "default"}
                    variant={r.cause === "recovery" ? "filled" : "outlined"}
                  />
                </TableCell>
                <TableCell sx={{ fontFamily: "monospace" }}>
                  {exitLabel(r)}
                </TableCell>
                <TableCell align="right">{r.generation ?? "—"}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </Box>
  );
}
