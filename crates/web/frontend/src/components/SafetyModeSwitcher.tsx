import LockIcon from "@mui/icons-material/Lock";
import ShieldIcon from "@mui/icons-material/Shield";
import WarningIcon from "@mui/icons-material/Warning";
import {
  Box,
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
  Typography,
} from "@mui/material";
import { alpha } from "@mui/material/styles";
import { useEffect, useState, type MouseEvent } from "react";
import {
  ELEVATION_DURATION_MS,
  safetyStripeBackground,
  useSafetyMode,
  type SafetyMode,
  type SafetyTier,
} from "./SafetyModeProvider";

export interface PeerElevation {
  /** Highest tier any other web session is currently in, or null if all
   *  peers are read-only. */
  tier: SafetyTier | null;
  writeCount: number;
  dangerousCount: number;
}

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

export function SafetyModeSwitcher({ peerElevation }: { peerElevation?: PeerElevation }) {
  const { mode, setMode, elevatedUntil } = useSafetyMode();
  const remaining = useRemaining(elevatedUntil);
  const [anchorEl, setAnchorEl] = useState<HTMLElement | null>(null);
  const [pendingDangerous, setPendingDangerous] = useState(false);

  // w[impl sessions.safety-mode]
  const peerTier = peerElevation?.tier ?? null;
  const peerWarning = (() => {
    if (!peerElevation || peerTier === null) return null;
    const parts: string[] = [];
    if (peerElevation.dangerousCount > 0) {
      parts.push(
        `${peerElevation.dangerousCount} other session${
          peerElevation.dangerousCount === 1 ? "" : "s"
        } in dangerous mode`,
      );
    }
    if (peerElevation.writeCount > 0) {
      parts.push(
        `${peerElevation.writeCount} other session${
          peerElevation.writeCount === 1 ? "" : "s"
        } in write mode`,
      );
    }
    return parts.join("; ");
  })();

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
    <Box sx={{ display: "flex", alignItems: "center" }}>
      {peerWarning && (
        <Tooltip title={`${peerWarning}. Coordinate before issuing your own changes.`}>
          <Chip
            icon={<WarningIcon fontSize="small" />}
            label={
              peerTier === "dangerous"
                ? `Peer in dangerous mode`
                : `Peer in write mode`
            }
            size="small"
            color={peerTier === "dangerous" ? "error" : "warning"}
            variant="outlined"
            sx={{
              mr: 1,
              fontFamily: "monospace",
              borderColor: peerTier === "dangerous" ? "error.light" : "warning.light",
              color: peerTier === "dangerous" ? "error.light" : "warning.light",
              "& .MuiChip-icon": {
                color: peerTier === "dangerous" ? "error.light" : "warning.light",
              },
            }}
          />
        </Tooltip>
      )}
      <Tooltip title={tooltip}>
        <Chip
          icon={<ModeIcon mode={mode} />}
          label={chipLabel}
          size="small"
          color={chipColor(mode)}
          onClick={openMenu}
          clickable
          variant={mode === "read" ? "outlined" : "filled"}
          sx={(theme) => ({
            mr: 1,
            pl: "0.5em",
            fontFamily: "monospace",
            ...(mode === "read" && {
              color: "common.white",
              borderColor: "rgba(255,255,255,0.5)",
              "& .MuiChip-icon": { color: "common.white" },
            }),
            // The active-tier indicator wears the same stripe pattern as the
            // dropdown items, but with stronger alpha so the stripes read
            // against the chip's already-coloured fill rather than washing
            // out into it.
            ...(mode !== "read" && {
              backgroundImage: safetyStripeBackground(theme, mode, {
                stripeAlpha: 0.5,
                gapAlpha: 0,
              }),
            }),
          })}
        />
      </Tooltip>
      <Menu anchorEl={anchorEl} open={!!anchorEl} onClose={closeMenu}>
        {peerWarning && (
          <Box
            sx={(theme) => ({
              px: 2,
              py: 1,
              maxWidth: 320,
              borderLeft: `3px solid ${
                peerTier === "dangerous"
                  ? theme.palette.error.main
                  : theme.palette.warning.main
              }`,
              backgroundColor:
                peerTier === "dangerous"
                  ? alpha(theme.palette.error.main, 0.08)
                  : alpha(theme.palette.warning.main, 0.08),
            })}
          >
            <Typography
              variant="caption"
              sx={{ display: "flex", alignItems: "center", gap: 0.5, fontWeight: 600 }}
              color={peerTier === "dangerous" ? "error" : "warning.dark"}
            >
              <WarningIcon fontSize="inherit" />
              Peer activity
            </Typography>
            <Typography variant="caption" sx={{ display: "block", color: "text.secondary" }}>
              {peerWarning}.
            </Typography>
          </Box>
        )}
        {(["read", "write", "dangerous"] as const).map((m) => {
          const secondary =
            m !== "read" && m === mode && remainingText !== null
              ? `${MODE_TOOLTIP[m]} · reverts in ${remainingText}`
              : m !== "read"
                ? `${MODE_TOOLTIP[m]} · auto-reverts after ${ELEVATION_MINUTES} min`
                : MODE_TOOLTIP[m];
          return (
            <MenuItem
              key={m}
              selected={m === mode}
              onClick={() => pick(m)}
              sx={(theme) =>
                m === "read"
                  ? {}
                  : {
                      backgroundImage: safetyStripeBackground(theme, m),
                      filter: "grayscale(0.8)",
                      transition: theme.transitions.create("filter", {
                        duration: theme.transitions.duration.shortest,
                      }),
                      "&:hover, &.Mui-selected, &.Mui-selected:hover": {
                        filter: "grayscale(0)",
                      },
                    }
              }
            >
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
    </Box>
  );
}
