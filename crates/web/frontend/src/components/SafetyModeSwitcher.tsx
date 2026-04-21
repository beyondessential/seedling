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
import { useState, type MouseEvent } from "react";
import { useSafetyMode, type SafetyMode } from "./SafetyModeProvider";

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

export function SafetyModeSwitcher() {
  const { mode, setMode } = useSafetyMode();
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

  return (
    <>
      <Tooltip title={MODE_TOOLTIP[mode]}>
        <Chip
          icon={<ModeIcon mode={mode} />}
          label={MODE_LABEL[mode]}
          size="small"
          color={chipColor(mode)}
          onClick={openMenu}
          clickable
          variant={mode === "read" ? "outlined" : "filled"}
          sx={{ mr: 1, fontFamily: "monospace" }}
        />
      </Tooltip>
      <Menu anchorEl={anchorEl} open={!!anchorEl} onClose={closeMenu}>
        {(["read", "write", "dangerous"] as const).map((m) => (
          <MenuItem key={m} selected={m === mode} onClick={() => pick(m)}>
            <ListItemIcon>
              <ModeIcon mode={m} />
            </ListItemIcon>
            <ListItemText primary={MODE_LABEL[m]} secondary={MODE_TOOLTIP[m]} />
          </MenuItem>
        ))}
      </Menu>
      <Dialog open={pendingDangerous} onClose={() => setPendingDangerous(false)} maxWidth="xs">
        <DialogTitle>Enable Dangerous mode?</DialogTitle>
        <DialogContent>
          <DialogContentText>
            Dangerous mode unlocks destructive actions such as deleting apps, volumes
            and keys, and terminating other users' sessions. These actions are
            irreversible or affect other operators — use with care.
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
