import AltRouteIcon from "@mui/icons-material/AltRoute";
import BackupIcon from "@mui/icons-material/Backup";
import CloudQueueIcon from "@mui/icons-material/CloudQueue";
import CloudUploadIcon from "@mui/icons-material/CloudUpload";
import DescriptionIcon from "@mui/icons-material/Description";
import EventNoteIcon from "@mui/icons-material/EventNote";
import HubIcon from "@mui/icons-material/Hub";
import HttpsIcon from "@mui/icons-material/Https";
import InventoryIcon from "@mui/icons-material/Inventory2";
import KeyIcon from "@mui/icons-material/Key";
import PeopleAltIcon from "@mui/icons-material/PeopleAlt";
import StorageIcon from "@mui/icons-material/Storage";
import { AppBar, Badge, Box, Chip, IconButton, Toolbar, Tooltip, Typography } from "@mui/material";
import { useCallback, useEffect, useMemo } from "react";
import { Link } from "react-router-dom";
import { useEventRefresh } from "../hooks/useEventRefresh";
import { useOiQuery } from "../hooks/useOi";
import { SafetyModeSwitcher } from "./SafetyModeSwitcher";
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
  ev.type === "WebSessionModeChanged" ||
  ev.type === "ShellStarted" ||
  ev.type === "ShellExited" ||
  ev.type === "ForwardStarted" ||
  ev.type === "ForwardStopped";

// w[impl routes.volumes.held-count]
// The backend emits HeldVolumeCreated whenever a volume is placed into the
// held state (app-script updates, backend-migration, operator-triggered site
// volume deletion), HeldVolumeDeleted when an operator confirms final
// removal, and HeldVolumeRestored when a held volume is restored as a fresh
// site volume. AppUpdated is kept as a belt-and-braces trigger for legacy
// clients/servers that predate the held-volume events.
const isHeldVolumeEvent = (ev: SeedlingEvent) =>
  ev.type === "HeldVolumeCreated" ||
  ev.type === "HeldVolumeDeleted" ||
  ev.type === "HeldVolumeRestored" ||
  ev.type === "AppUpdated";

export function Navbar() {
  const { data } = useOiQuery<StatusSummary>("/server/status", {});
  // Surface the server's hostname in the browser tab title, so an operator
  // with multiple Seedling tabs open can tell at a glance which site each
  // one is pointing at without flicking through them.
  useEffect(() => {
    document.title = data?.hostname ? `${data.hostname} · Seedling` : "Seedling";
  }, [data?.hostname]);
  const { data: faults, refetch: refetchFaults } = useOiQuery<FaultRecord[]>("/faults/list", {});
  const { data: clients, refetch: refetchClients } =
    useOiQuery<ConnectedClients>("/connected-clients/list", {});
  const { data: heldVols, refetch: refetchHeld } =
    useOiQuery<HeldVolume[]>("/volumes/held/list", {});
  const { reconnecting, sidebarOpen, setSidebarOpen, webSessionId } = useSessionContext();

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

  // w[impl sessions.safety-mode]
  // Compute the highest safety tier any *other* web session is currently in.
  // We exclude our own session (identified by webSessionId, set after the
  // first heartbeat round-trip) so promoting our own mode does not flag the
  // navbar against ourselves.
  const peerElevation = useMemo<{
    tier: "write" | "dangerous" | null;
    writeCount: number;
    dangerousCount: number;
  }>(() => {
    const peers = (clients?.web ?? []).filter((s) => s.id !== webSessionId);
    let writeCount = 0;
    let dangerousCount = 0;
    for (const s of peers) {
      if (s.safety_mode === "dangerous") dangerousCount++;
      else if (s.safety_mode === "write") writeCount++;
    }
    const tier: "write" | "dangerous" | null =
      dangerousCount > 0 ? "dangerous" : writeCount > 0 ? "write" : null;
    return { tier, writeCount, dangerousCount };
  }, [clients?.web, webSessionId]);

  const sessionsBadgeColor: "primary" | "warning" | "error" =
    peerElevation.tier === "dangerous"
      ? "error"
      : peerElevation.tier === "write"
        ? "warning"
        : "primary";

  const sessionsTooltip = peerElevation.tier
    ? `${sessionCount} connected client${sessionCount === 1 ? "" : "s"} · ${
        peerElevation.dangerousCount > 0
          ? `${peerElevation.dangerousCount} in dangerous mode`
          : ""
      }${
        peerElevation.dangerousCount > 0 && peerElevation.writeCount > 0 ? ", " : ""
      }${
        peerElevation.writeCount > 0
          ? `${peerElevation.writeCount} in write mode`
          : ""
      }`
    : `${sessionCount} connected client${sessionCount === 1 ? "" : "s"}`;

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
          sx={{ fontWeight: 700, letterSpacing: "-0.5px", color: "inherit", textDecoration: "none", mr: 2 }}
        >
          Seedling
        </Typography>
        <Tooltip title={sessionsTooltip}>
          <IconButton
            size="small"
            component={Link}
            to="/"
            sx={{
              color: peerElevation.tier
                ? peerElevation.tier === "dangerous"
                  ? "error.light"
                  : "warning.light"
                : "rgba(255,255,255,0.6)",
              mr: 0.5,
            }}
          >
            <Badge
              badgeContent={sessionCount}
              color={sessionsBadgeColor}
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
        <Tooltip title="Canopy enrolment">
          <IconButton
            size="small"
            component={Link}
            to="/canopy"
            sx={{ color: "rgba(255,255,255,0.6)", mr: 0.5 }}
          >
            <CloudUploadIcon fontSize="small" />
          </IconButton>
        </Tooltip>
        <Tooltip title="Container images">
          <IconButton
            size="small"
            component={Link}
            to="/images"
            sx={{ color: "rgba(255,255,255,0.6)", mr: 0.5 }}
          >
            <InventoryIcon fontSize="small" />
          </IconButton>
        </Tooltip>
        <Tooltip title="Services">
          <IconButton
            size="small"
            component={Link}
            to="/services"
            sx={{ color: "rgba(255,255,255,0.6)", mr: 0.5 }}
          >
            <HubIcon fontSize="small" />
          </IconButton>
        </Tooltip>
        <Tooltip title="Site ingresses">
          <IconButton
            size="small"
            component={Link}
            to="/ingresses"
            sx={{ color: "rgba(255,255,255,0.6)", mr: 0.5 }}
          >
            <AltRouteIcon fontSize="small" />
          </IconButton>
        </Tooltip>
        <Tooltip title="TLS certificates">
          <IconButton
            size="small"
            component={Link}
            to="/certificates"
            sx={{ color: "rgba(255,255,255,0.6)", mr: 0.5 }}
          >
            <HttpsIcon fontSize="small" />
          </IconButton>
        </Tooltip>
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
        <SafetyModeSwitcher peerElevation={peerElevation} />
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
