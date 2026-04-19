import RefreshIcon from "@mui/icons-material/Refresh";
import {
  Alert,
  Box,
  CircularProgress,
  IconButton,
  Stack,
  Tooltip,
  Typography,
} from "@mui/material";
import { useCallback } from "react";
import { Link } from "react-router-dom";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useOiQuery } from "../hooks/useOi";
import { useEventRefresh } from "../hooks/useEventRefresh";
import type { FaultRecord, SeedlingEvent } from "../lib/types";

export default function Faults() {
  const { data, loading, error, refetch } =
    useOiQuery<FaultRecord[]>("/faults/list", {});
  const matchesFaults = useCallback((ev: SeedlingEvent) => ev.type === "FaultFiled" || ev.type === "FaultCleared", []);
  useEventRefresh(refetch, matchesFaults);

  return (
    <Box sx={{ p: 3, maxWidth: 900, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          Active Faults
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

      {data && data.length === 0 && (
        <Typography color="text.secondary">No active faults.</Typography>
      )}

      {data && data.length > 0 && (
        <Stack spacing={1}>
          {data.map((f) => (
            <Alert key={f.id} severity="error" sx={{ fontFamily: "monospace" }}>
              <Box sx={{ display: "flex", justifyContent: "space-between", gap: 2, flexWrap: "wrap" }}>
                <Box>
                  {f.app && (
                    <>
                      <Link to={`/apps/${f.app}`} style={{ color: "inherit", fontWeight: 600 }}>
                        {f.app}
                      </Link>
                      {" · "}
                    </>
                  )}
                  <strong>{f.kind}</strong>
                  {f.resource_name && ` · ${f.resource_type}/${f.resource_name}`}
                  {f.instance_id && ` (${f.instance_id.slice(0, 12)})`}
                  {" — "}
                  {f.description}
                </Box>
                <Typography variant="caption" color="text.secondary" sx={{ whiteSpace: "nowrap", alignSelf: "center" }}>
                  {new Date(f.timestamp).toLocaleString()}
                </Typography>
              </Box>
            </Alert>
          ))}
        </Stack>
      )}
    </Box>
  );
}
