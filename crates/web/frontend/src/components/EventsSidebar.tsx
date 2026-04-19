import { useCallback, useEffect, useRef } from "react";
import { Link } from "react-router-dom";
import {
  Box,
  Chip,
  Divider,
  Paper,
  Tooltip,
  Typography,
} from "@mui/material";
import { useSessionContext } from "./SessionProvider";
import type { SeedlingEvent } from "../lib/types";

const MIN_WIDTH = 220;
const MAX_WIDTH = 700;

function eventColor(type: string): "default" | "success" | "error" | "warning" | "info" {
  if (type.includes("Fault")) return "error";
  if (type.includes("Failed") || type.includes("Exited")) return "warning";
  if (type.includes("Completed") || type.includes("Registered")) return "success";
  if (type.includes("Started") || type.includes("Changed") || type.includes("State")) return "info";
  return "default";
}

function eventSummary(ev: SeedlingEvent): string {
  switch (ev.type) {
    case "AppRegistered": return `registered (gen ${ev.generation ?? "?"})`;
    case "AppDeregistered": return "deregistered";
    case "AppUpdated": return `updated to gen ${ev.generation ?? "?"}`;
    case "ParamSet": return `param ${ev.name ?? ""} set`;
    case "ParamUnset": return `param ${ev.name ?? ""} unset`;
    case "OperationStarted": return `${ev.action_name ?? "operation"} started`;
    case "OperationCompleted": return `${ev.action_name ?? "operation"} completed`;
    case "OperationFailed": return `${ev.action_name ?? "operation"} failed: ${ev.error ?? ""}`;
    case "FaultFiled": return `fault: ${ev.kind ?? ""} — ${ev.description ?? ""}`;
    case "FaultCleared": return `fault cleared: ${ev.kind ?? ""}`;
    case "ResourceStateChanged": return `${ev.resource_type ?? ""}/${ev.resource_name ?? ""} → ${ev.state ?? ""}`;
    case "ScaleChanged": return `${ev.deployment ?? ""} scaled to ${ev.scale ?? "?"}`;
    case "ForwardStarted": return `forward :${ev.port ?? "?"} started`;
    case "ForwardStopped": return `forward stopped`;
    case "ShellExited": return `shell exited (${ev.exit_code ?? "?"})`;
    case "ServerBusy": return ev.reason ?? "server busy";
    default: return "";
  }
}

function EventRow({ ev }: { ev: SeedlingEvent }) {
  const ts = new Date(ev.timestamp);
  const timeStr = ts.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });

  return (
    <Box sx={{ px: 1.5, py: 0.75, borderBottom: "1px solid", borderColor: "divider" }}>
      <Box sx={{ display: "flex", alignItems: "center", gap: 0.5, mb: 0.25 }}>
        <Chip
          label={ev.type.replace(/([A-Z])/g, " $1").trim()}
          size="small"
          color={eventColor(ev.type)}
          variant="outlined"
          sx={{ fontSize: "0.65rem", height: 18, "& .MuiChip-label": { px: 0.75 } }}
        />
        <Typography variant="caption" color="text.disabled" sx={{ ml: "auto", whiteSpace: "nowrap" }}>
          {timeStr}
        </Typography>
      </Box>
      {ev.app && (
        <Typography
          variant="caption"
          component={Link}
          to={`/apps/${ev.app}`}
          sx={{ color: "text.secondary", textDecoration: "none", "&:hover": { textDecoration: "underline" } }}
        >
          {ev.app}
        </Typography>
      )}
      {eventSummary(ev) && (
        <Typography
          variant="caption"
          display="block"
          color="text.secondary"
          sx={{ fontFamily: "monospace", fontSize: "0.72rem", wordBreak: "break-all" }}
        >
          {eventSummary(ev)}
        </Typography>
      )}
    </Box>
  );
}

export function EventsSidebar() {
  const { events, sidebarWidth, setSidebarWidth } = useSessionContext();
  const dragging = useRef(false);
  const startX = useRef(0);
  const startWidth = useRef(0);

  const onMouseDown = useCallback((e: React.MouseEvent) => {
    dragging.current = true;
    startX.current = e.clientX;
    startWidth.current = sidebarWidth;
    e.preventDefault();
  }, [sidebarWidth]);

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!dragging.current) return;
      const delta = startX.current - e.clientX;
      const next = Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, startWidth.current + delta));
      setSidebarWidth(next);
    };
    const onUp = () => { dragging.current = false; };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [setSidebarWidth]);

  return (
    <Paper
      variant="outlined"
      square
      sx={{
        width: sidebarWidth,
        flexShrink: 0,
        display: "flex",
        flexDirection: "column",
        position: "relative",
        borderTop: "none",
        borderBottom: "none",
        borderRight: "none",
        overflow: "hidden",
      }}
    >
      {/* Drag handle */}
      <Box
        onMouseDown={onMouseDown}
        sx={{
          position: "absolute",
          left: 0,
          top: 0,
          bottom: 0,
          width: 4,
          cursor: "col-resize",
          zIndex: 1,
          "&:hover": { bgcolor: "primary.main", opacity: 0.4 },
        }}
      />

      <Box sx={{ px: 1.5, py: 0.75, display: "flex", alignItems: "center" }}>
        <Typography variant="subtitle2" sx={{ fontWeight: 700, flexGrow: 1 }}>
          Events
        </Typography>
        <Tooltip title={`${events.length} event${events.length === 1 ? "" : "s"} cached`}>
          <Typography variant="caption" color="text.disabled">
            {events.length}
          </Typography>
        </Tooltip>
      </Box>

      <Divider />

      <Box sx={{ flexGrow: 1, overflow: "auto" }}>
        {events.length === 0 ? (
          <Typography variant="caption" color="text.disabled" sx={{ display: "block", p: 1.5 }}>
            No events yet.
          </Typography>
        ) : (
          events.map((ev, i) => <EventRow key={i} ev={ev} />)
        )}
      </Box>
    </Paper>
  );
}
