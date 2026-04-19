import EventNoteIcon from "@mui/icons-material/EventNote";
import PeopleAltIcon from "@mui/icons-material/PeopleAlt";
import StorageIcon from "@mui/icons-material/Storage";
import { AppBar, Badge, Box, Chip, IconButton, Toolbar, Tooltip, Typography } from "@mui/material";
import { useCallback } from "react";
import { Link } from "react-router-dom";
import { useEventRefresh } from "../hooks/useEventRefresh";
import { useOiQuery } from "../hooks/useOi";
import { useSessionContext } from "./SessionProvider";
import type { ConnectedClients, FaultRecord, SeedlingEvent } from "../lib/types";

interface StatusSummary {
  hostname: string;
  version: string;
}

const isFaultEvent = (ev: SeedlingEvent) =>
  ev.type === "FaultFiled" || ev.type === "FaultCleared";

const isSessionEvent = (ev: SeedlingEvent) =>
  ev.type === "WebSessionStarted" ||
  ev.type === "WebSessionStopped" ||
  ev.type === "ShellStarted" ||
  ev.type === "ShellExited" ||
  ev.type === "ForwardStarted" ||
  ev.type === "ForwardStopped";

export function Navbar() {
  const { data } = useOiQuery<StatusSummary>("/server/status", {});
  const { data: faults, refetch: refetchFaults } = useOiQuery<FaultRecord[]>("/faults/list", {});
  const { data: clients, refetch: refetchClients } =
    useOiQuery<ConnectedClients>("/connected-clients/list", {});
  const { reconnecting, sidebarOpen, setSidebarOpen } = useSessionContext();

  const matchFaults = useCallback(isFaultEvent, []);
  const matchSessions = useCallback(isSessionEvent, []);
  useEventRefresh(refetchFaults, matchFaults);
  useEventRefresh(refetchClients, matchSessions);

  const faultCount = faults?.length ?? 0;
  const sessionCount =
    (clients?.web.length ?? 0) +
    (clients?.shells.length ?? 0) +
    (clients?.forwards.length ?? 0);

  return (
    <AppBar position="fixed">
      <Toolbar variant="dense">
        <Typography
          component={Link}
          to="/"
          sx={{ mr: 1, fontSize: "1.2rem", lineHeight: 1, textDecoration: "none" }}
        >
          🌱
        </Typography>
        <Typography
          variant="h6"
          component={Link}
          to="/"
          sx={{ fontWeight: 700, letterSpacing: "-0.5px", color: "inherit", textDecoration: "none" }}
        >
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
        <Tooltip title="Volumes">
          <IconButton
            size="small"
            component={Link}
            to="/volumes"
            sx={{ color: "rgba(255,255,255,0.6)", mr: 0.5 }}
          >
            <StorageIcon fontSize="small" />
          </IconButton>
        </Tooltip>
        <Tooltip title={`${sessionCount} connected client${sessionCount === 1 ? "" : "s"}`}>
          <IconButton
            size="small"
            component={Link}
            to="/sessions"
            sx={{ color: "rgba(255,255,255,0.6)", mr: 0.5 }}
          >
            <Badge
              badgeContent={sessionCount}
              color="primary"
              sx={{
                "& .MuiBadge-badge": {
                  fontSize: "0.6rem",
                  minWidth: 14,
                  height: 14,
                  padding: "0 3px",
                },
              }}
            >
              <PeopleAltIcon fontSize="small" />
            </Badge>
          </IconButton>
        </Tooltip>
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
