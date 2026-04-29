import UpgradeIcon from "@mui/icons-material/Upgrade";
import {
  Box,
  Button,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Stack,
  TextField,
  Tooltip,
  Typography,
} from "@mui/material";
import { useState } from "react";
import { useOiAction } from "../hooks/useOiAction";
import { OiErrorAlert } from "./OiErrorAlert";
import { useGuard } from "./SafetyModeProvider";

/// Promote a read-only snapshot site volume to a fresh read-write managed
/// site volume. Backed by /volumes/site/promote. The source snapshot is
/// unaffected and may be deleted independently later.
///
/// Render with `key={source}` so switching which snapshot is being
/// promoted remounts the dialog and regenerates the suggested default name.
export function PromoteSnapshotDialog({
  onClose,
  source,
  onSuccess,
}: {
  onClose: () => void;
  /** Name of the source snapshot site volume. */
  source: string;
  onSuccess: () => void;
}) {
  const { execute, loading, error, clearError } = useOiAction();
  const writeGuard = useGuard("write");
  const [name, setName] = useState(() => defaultPromotedName(source));

  const handleClose = () => {
    clearError();
    onClose();
  };

  const handleSubmit = async () => {
    if (!name.trim()) return;
    const result = await execute("/volumes/site/promote", {
      source,
      name: name.trim(),
    });
    if (result !== null) {
      onSuccess();
      handleClose();
    }
  };

  return (
    <Dialog open onClose={handleClose} maxWidth="xs" fullWidth>
      <DialogTitle>Promote snapshot</DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 0.5 }}>
          {error && <OiErrorAlert error={error} />}
          <Typography variant="body2" sx={{
            color: "text.secondary"
          }}>
            Create a fresh read-write managed site volume seeded from{" "}
            <Box component="span" sx={{ fontFamily: "monospace" }}>
              {source}
            </Box>
            . The source snapshot remains available.
          </Typography>
          <TextField
            label="New volume name"
            size="small"
            value={name}
            onChange={(e) => setName(e.target.value)}
            autoFocus
            helperText="Must be unique across site volumes"
            slotProps={{
              htmlInput: { style: { fontFamily: "monospace" } }
            }}
          />
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={handleClose} disabled={loading}>
          Cancel
        </Button>
        <Tooltip title={writeGuard.title()}>
          <span>
            <Button
              variant="contained"
              startIcon={<UpgradeIcon />}
              onClick={() => void handleSubmit()}
              disabled={loading || !name.trim() || !writeGuard.allowed}
            >
              {loading ? "Promoting…" : "Promote"}
            </Button>
          </span>
        </Tooltip>
      </DialogActions>
    </Dialog>
  );
}

function defaultPromotedName(source: string): string {
  const trimmed = source.replace(/-snapshot$/, "").replace(/-\d{8}-\d{6}$/, "");
  return `${trimmed}-promoted`;
}
