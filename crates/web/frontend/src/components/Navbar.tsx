import BackupIcon from "@mui/icons-material/Backup";
import CloudQueueIcon from "@mui/icons-material/CloudQueue";
import DescriptionIcon from "@mui/icons-material/Description";
import EventNoteIcon from "@mui/icons-material/EventNote";
import KeyIcon from "@mui/icons-material/Key";
import PeopleAltIcon from "@mui/icons-material/PeopleAlt";
import StorageIcon from "@mui/icons-material/Storage";
import { AppBar, Badge, Box, Chip, IconButton, Toolbar, Tooltip, Typography } from "@mui/material";
import { useCallback } from "react";
import { Link } from "react-router-dom";
import { useEventRefresh } from "../hooks/useEventRefresh";
import { useOiQuery } from "../hooks/useOi";
import { useSessionContext } from "./SessionProvider";
import type { ConnectedClients, FaultRecord, HeldVolume, SeedlingEvent } from "../lib/types";

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

// w[impl routes.volumes.held-count]
// The backend emits HeldVolumeCreated whenever a volume is placed into the
// held state (app-script updates, backend-migration, operator-triggered site
// volume deletion) and HeldVolumeDeleted when an operator confirms final
// removal. AppUpdated is kept as a belt-and-braces trigger for legacy
// clients/servers that predate the held-volume events.
const isHeldVolumeEvent = (ev: SeedlingEvent) =>
  ev.type === "HeldVolumeCreated" ||
  ev.type === "HeldVolumeDeleted" ||
  ev.type === "AppUpdated";

export function Navbar() {
  const { data } = useOiQuery<StatusSummary>("/server/status", {});
  const { data: faults, refetch: refetchFaults } = useOiQuery<FaultRecord[]>("/faults/list", {});
  const { data: clients, refetch: refetchClients } =
    useOiQuery<ConnectedClients>("/connected-clients/list", {});
  const { data: heldVols, refetch: refetchHeld } =
    useOiQuery<HeldVolume[]>("/volumes/held/list", {});
  const { reconnecting, sidebarOpen, setSidebarOpen } = useSessionContext();

  const matchFaults = useCallback(isFaultEvent, []);
  const matchSessions = useCallback(isSessionEvent, []);
  const matchHeldVolumes = useCallback(isHeldVolumeEvent, []);
  useEventRefresh(refetchFaults, matchFaults);
  useEventRefresh(refetchClients, matchSessions);
  useEventRefresh(refetchHeld, matchHeldVolumes);

  const faultCount = faults?.length ?? 0;
  const heldCount = heldVols?.length ?? 0;
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
        <Tooltip
          title={
            heldCount > 0
              ? `Volumes · ${heldCount} held volume${heldCount === 1 ? "" : "s"} pending review`
              : "Volumes"
          }
        >
          <IconButton
            size="small"
            component={Link}
            to="/volumes"
            sx={{ color: "rgba(255,255,255,0.6)", mr: 0.5 }}
          >
            <Badge
              badgeContent={heldCount}
              color="warning"
              sx={{
                "& .MuiBadge-badge": {
                  fontSize: "0.6rem",
                  minWidth: 14,
                  height: 14,
                  padding: "0 3px",
                },
              }}
            >
              <StorageIcon fontSize="small" />
            </Badge>
          </IconButton>
        </Tooltip>
        <Tooltip title="Templates">
          <IconButton
            size="small"
            component={Link}
            to="/templates"
            sx={{ color: "rgba(255,255,255,0.6)", mr: 0.5 }}
          >
            <DescriptionIcon fontSize="small" />
          </IconButton>
        </Tooltip>
        <Tooltip title="Backups">
          <IconButton
            size="small"
            component={Link}
            to="/backups"
            sx={{ color: "rgba(255,255,255,0.6)", mr: 0.5 }}
          >
            <BackupIcon fontSize="small" />
          </IconButton>
        </Tooltip>
        <Tooltip title="Container registry allowlist">
          <IconButton
            size="small"
            component={Link}
            to="/registries"
            sx={{ color: "rgba(255,255,255,0.6)", mr: 0.5 }}
          >
            <CloudQueueIcon fontSize="small" />
          </IconButton>
        </Tooltip>
        <Tooltip title="Authorised OI keys">
          <IconButton
            size="small"
            component={Link}
            to="/keys"
            sx={{ color: "rgba(255,255,255,0.6)", mr: 0.5 }}
          >
            <KeyIcon fontSize="small" />
          </IconButton>
        </Tooltip>
        <Tooltip title={`${sessionCount} connected client${sessionCount === 1 ? "" : "s"}`}>
          <IconButton
            size="small"
            component={Link}
            to="/"
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
