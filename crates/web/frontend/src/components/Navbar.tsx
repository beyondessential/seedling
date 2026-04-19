import EventNoteIcon from "@mui/icons-material/EventNote";
import { AppBar, Box, Chip, IconButton, Toolbar, Tooltip, Typography } from "@mui/material";
import { Link } from "react-router-dom";
import { useOiQuery } from "../hooks/useOi";
import { useSessionContext } from "./SessionProvider";
import type { FaultRecord } from "../lib/types";

interface StatusSummary {
  hostname: string;
  version: string;
}

export function Navbar() {
  const { data } = useOiQuery<StatusSummary>("/server/status", {});
  const { data: faults } = useOiQuery<FaultRecord[]>("/faults/list", {});
  const { reconnecting, sidebarOpen, setSidebarOpen } = useSessionContext();
  const faultCount = faults?.length ?? 0;

  return (
    <AppBar position="fixed">
      <Toolbar variant="dense">
        <Typography sx={{ mr: 1, fontSize: "1.2rem", lineHeight: 1 }}>
          🌱
        </Typography>
        <Typography variant="h6" sx={{ fontWeight: 700, letterSpacing: "-0.5px" }}>
          Seedling
        </Typography>
        <Box sx={{ flexGrow: 1 }} />
        {faultCount > 0 && (
          <Tooltip title={`${faultCount} active fault${faultCount === 1 ? "" : "s"}`}>
            <Chip
              label={`${faultCount} fault${faultCount === 1 ? "" : "s"}`}
              size="small"
              color="error"
              component={Link}
              to="/faults"
              clickable
              sx={{ mr: 1, fontFamily: "monospace" }}
            />
          </Tooltip>
        )}
        {reconnecting && (
          <Chip
            label="Reconnecting…"
            size="small"
            color="warning"
            sx={{ mr: 1, fontFamily: "monospace" }}
          />
        )}
        {data?.hostname && (
          <Typography variant="body2" sx={{ opacity: 0.85, mr: 1, fontFamily: "monospace" }}>
            {data.hostname}
          </Typography>
        )}
        <Tooltip title={sidebarOpen ? "Hide events" : "Show events"}>
          <IconButton
            color={sidebarOpen ? "inherit" : "default"}
            size="small"
            onClick={() => setSidebarOpen(!sidebarOpen)}
            sx={{ color: sidebarOpen ? "white" : "rgba(255,255,255,0.6)" }}
          >
            <EventNoteIcon fontSize="small" />
          </IconButton>
        </Tooltip>
      </Toolbar>
    </AppBar>
  );
}
