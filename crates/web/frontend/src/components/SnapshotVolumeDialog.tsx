import CameraAltIcon from "@mui/icons-material/CameraAlt";
import {
  Box,
  Button,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Stack,
  TextField,
  Typography,
} from "@mui/material";
import { useState } from "react";
import { useOiAction } from "../hooks/useOiAction";
import { SolidActionButton } from "./ActionButton";
import { OiErrorAlert } from "./OiErrorAlert";

/// Take a point-in-time snapshot of a volume, landing it as a new managed
/// site volume of kind=Snapshot (read-only at the filesystem level on
/// BTRFS). Backed by /volumes/site/snapshot; the operator picks a name for
/// the snapshot and the backend records its provenance.
///
/// Render with `key={source}` so switching which volume is being
/// snapshotted remounts the dialog and regenerates the timestamped default
/// name — otherwise an operator opening the dialog for one volume, then
/// another, would see the first volume's suggestion still in the field.
export function SnapshotVolumeDialog({
  onClose,
  source,
  sourceLabel,
  onSuccess,
}: {
  onClose: () => void;
  /** Source volume id: `_site/<name>` or `<app>/<volume>`. */
  source: string;
  /** Human-readable source label for the dialog body. */
  sourceLabel: string;
  onSuccess: () => void;
}) {
  const { execute, loading, error, clearError } = useOiAction();
  const [name, setName] = useState(() => defaultSnapshotName(source));

  const handleClose = () => {
    clearError();
    onClose();
  };

  const handleSubmit = async () => {
    if (!name.trim()) return;
    const result = await execute("/volumes/site/snapshot", {
      name: name.trim(),
      source,
    });
    if (result !== null) {
      onSuccess();
      handleClose();
    }
  };

  return (
    <Dialog open onClose={handleClose} maxWidth="xs" fullWidth>
      <DialogTitle>Snapshot volume</DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 0.5 }}>
          {error && <OiErrorAlert error={error} />}
          <Typography variant="body2" sx={{
            color: "text.secondary"
          }}>
            Capture a point-in-time copy of{" "}
            <Box component="span" sx={{ fontFamily: "monospace" }}>
              {sourceLabel}
            </Box>{" "}
            as a new read-only site volume.
          </Typography>
          <TextField
            label="Snapshot name"
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
        <SolidActionButton
          safety="write"
          startIcon={<CameraAltIcon />}
          onClick={() => void handleSubmit()}
          disabled={loading || !name.trim()}
        >
          {loading ? "Snapshotting…" : "Snapshot"}
        </SolidActionButton>
      </DialogActions>
    </Dialog>
  );
}

/// Build a default snapshot name like `<source>-20260421-134530`.
function defaultSnapshotName(source: string): string {
  const now = new Date();
  const pad = (n: number) => n.toString().padStart(2, "0");
  const stamp =
    `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}` +
    `-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  const safe = source.replace(/[^a-zA-Z0-9._-]/g, "-").replace(/^-+|-+$/g, "");
  return `${safe}-${stamp}`;
}
