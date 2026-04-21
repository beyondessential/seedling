import LockIcon from "@mui/icons-material/Lock";
import ShieldIcon from "@mui/icons-material/Shield";
import WarningIcon from "@mui/icons-material/Warning";
import {
  Button,
  Chip,
  Dialog,
  DialogActions,
  DialogContent,
  DialogContentText,
  DialogTitle,
  ListItemIcon,
  ListItemText,
  Menu,
  MenuItem,
  Tooltip,
} from "@mui/material";
import { useEffect, useState, type MouseEvent } from "react";
import {
  ELEVATION_DURATION_MS,
  useSafetyMode,
  type SafetyMode,
} from "./SafetyModeProvider";

const MODE_LABEL: Record<SafetyMode, string> = {
  read: "Read-only",
  write: "Write",
  dangerous: "Dangerous",
};

const MODE_TOOLTIP: Record<SafetyMode, string> = {
  read: "Read-only: mutating actions are disabled",
  write: "Write: routine mutations enabled; destructive actions still blocked",
  dangerous: "Dangerous: all actions including destructive ones are enabled",
};

const ELEVATION_MINUTES = Math.round(ELEVATION_DURATION_MS / 60_000);

function ModeIcon({ mode }: { mode: SafetyMode }) {
  if (mode === "read") return <LockIcon fontSize="small" />;
  if (mode === "write") return <ShieldIcon fontSize="small" />;
  return <WarningIcon fontSize="small" />;
}

function chipColor(mode: SafetyMode): "default" | "warning" | "error" {
  if (mode === "read") return "default";
  if (mode === "write") return "warning";
  return "error";
}

function formatRemaining(ms: number): string {
  if (ms <= 0) return "0s";
  const totalSeconds = Math.ceil(ms / 1000);
  if (totalSeconds <= 59) return `${totalSeconds}s`;
  return `${Math.round(ms / 60_000)}m`;
}

function useRemaining(until: number | null): number | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (until === null) return;
    const id = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(id);
  }, [until]);
  return until === null ? null : Math.max(0, until - now);
}

export function SafetyModeSwitcher() {
  const { mode, setMode, elevatedUntil } = useSafetyMode();
  const remaining = useRemaining(elevatedUntil);
  const [anchorEl, setAnchorEl] = useState<HTMLElement | null>(null);
  const [pendingDangerous, setPendingDangerous] = useState(false);

  const openMenu = (e: MouseEvent<HTMLElement>) => setAnchorEl(e.currentTarget);
  const closeMenu = () => setAnchorEl(null);

  const pick = (next: SafetyMode) => {
    closeMenu();
    if (next === "dangerous" && mode !== "dangerous") {
      setPendingDangerous(true);
      return;
    }
    setMode(next);
  };

  const confirmDangerous = () => {
    setPendingDangerous(false);
    setMode("dangerous");
  };

  const remainingText = remaining !== null ? formatRemaining(remaining) : null;
  const chipLabel =
    remainingText !== null && mode !== "read"
      ? `${MODE_LABEL[mode]} · ${remainingText}`
      : MODE_LABEL[mode];
  const tooltip =
    remainingText !== null && mode !== "read"
      ? `${MODE_TOOLTIP[mode]}. Auto-reverts to read-only in ${remainingText}.`
      : MODE_TOOLTIP[mode];

  return (
    <>
      <Tooltip title={tooltip}>
        <Chip
          icon={<ModeIcon mode={mode} />}
          label={chipLabel}
          size="small"
          color={chipColor(mode)}
          onClick={openMenu}
          clickable
          variant={mode === "read" ? "outlined" : "filled"}
          sx={{
            mr: 1,
            fontFamily: "monospace",
            "& .MuiChip-icon": { ml: "0.5em" },
          }}
        />
      </Tooltip>
      <Menu anchorEl={anchorEl} open={!!anchorEl} onClose={closeMenu}>
        {(["read", "write", "dangerous"] as const).map((m) => {
          const secondary =
            m !== "read" && m === mode && remainingText !== null
              ? `${MODE_TOOLTIP[m]} · reverts in ${remainingText}`
              : m !== "read"
                ? `${MODE_TOOLTIP[m]} · auto-reverts after ${ELEVATION_MINUTES} min`
                : MODE_TOOLTIP[m];
          return (
            <MenuItem key={m} selected={m === mode} onClick={() => pick(m)}>
              <ListItemIcon>
                <ModeIcon mode={m} />
              </ListItemIcon>
              <ListItemText primary={MODE_LABEL[m]} secondary={secondary} />
            </MenuItem>
          );
        })}
      </Menu>
      <Dialog open={pendingDangerous} onClose={() => setPendingDangerous(false)} maxWidth="xs">
        <DialogTitle>Enable Dangerous mode?</DialogTitle>
        <DialogContent>
          <DialogContentText>
            Dangerous mode unlocks destructive actions such as deleting apps, volumes
            and keys, and terminating other users' sessions. These actions are
            irreversible or affect other operators — use with care. Dangerous mode
            auto-reverts to read-only after {ELEVATION_MINUTES} minutes.
          </DialogContentText>
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setPendingDangerous(false)}>Cancel</Button>
          <Button onClick={confirmDangerous} color="error" variant="contained">
            Enable Dangerous mode
          </Button>
        </DialogActions>
      </Dialog>
    </>
  );
}
