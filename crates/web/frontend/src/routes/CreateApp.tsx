import {
  Box,
  Button,
  CircularProgress,
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
import { Link, useNavigate } from "react-router-dom";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useGuard } from "../components/SafetyModeProvider";
import { ScriptEditor } from "../components/ScriptEditor";
import { ScriptInventory } from "../components/ScriptInventory";
import { useOiAction } from "../hooks/useOiAction";
import type { TemplatePreview } from "../lib/types";

const NAME_RE = /^[a-zA-Z][a-zA-Z0-9-]{1,61}[a-zA-Z0-9]$/;

function nameError(name: string): string | null {
  if (name.length === 0) return null;
  if (name.length < 3) return "Name must be at least 3 characters.";
  if (name.length > 63) return "Name must be at most 63 characters.";
  if (!NAME_RE.test(name))
    return "Name must start with a letter, end with a letter or digit, and contain only letters, digits, or hyphens.";
  return null;
}

export default function CreateApp() {
  const navigate = useNavigate();
  const { execute: previewExec, loading: previewing, error: previewError } = useOiAction();
  const { execute: createExec, loading: creating, error: createError } = useOiAction();
  const writeGuard = useGuard("write");
  const [name, setName] = useState("");
  const [script, setScript] = useState("");
  const [nameTouched, setNameTouched] = useState(false);
  const [preview, setPreview] = useState<TemplatePreview | null>(null);

  const validationError = nameError(name);
  const canReview =
    name.length > 0 && validationError === null && script.length > 0 && !previewing && !creating;

  const handleReview = async () => {
    if (!canReview) return;
    try {
      const result = (await previewExec("/templates/preview", { body: script })) as TemplatePreview;
      setPreview(result);
    } catch {
      // displayed via previewError
    }
  };

  const handleConfirm = async () => {
    try {
      await createExec("/apps/create", { app: name, script });
      navigate(`/apps/${name}`);
    } catch {
      // displayed via createError
    }
  };

  const handleCancel = () => {
    setPreview(null);
  };

  return (
    <Box
      sx={{
        p: 3,
        maxWidth: 960,
        mx: "auto",
        display: "flex",
        flexDirection: "column",
        gap: 2,
      }}
    >
      <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
        <Typography
          component={Link}
          to="/"
          variant="body2"
          sx={{
            color: "text.secondary",
            textDecoration: "none",
            "&:hover": { textDecoration: "underline" },
          }}
        >
          Apps
        </Typography>
        <Typography variant="body2" color="text.disabled">
          /
        </Typography>
        <Typography variant="body2">New app</Typography>
        <Box sx={{ flexGrow: 1 }} />
        <Button size="small" component={Link} to="/" disabled={previewing || creating}>
          Cancel
        </Button>
        <Tooltip title={writeGuard.reason ?? ""}>
          <span>
            <Button
              size="small"
              variant="contained"
              onClick={handleReview}
              disabled={!canReview || !writeGuard.allowed}
            >
              {previewing ? "Previewing…" : "Review & create"}
            </Button>
          </span>
        </Tooltip>
      </Box>

      <Stack spacing={2}>
        {previewError && <OiErrorAlert error={previewError} />}

        <TextField
          label="App name"
          size="small"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => setNameTouched(true)}
          error={nameTouched && validationError !== null}
          helperText={nameTouched ? (validationError ?? " ") : " "}
          inputProps={{ style: { fontFamily: "monospace" } }}
          sx={{ maxWidth: 400 }}
          autoFocus
        />

        <ScriptEditor value={script} onChange={setScript} />
      </Stack>

      <Dialog
        open={preview !== null}
        onClose={() => !creating && handleCancel()}
        maxWidth="md"
        fullWidth
      >
        <DialogTitle>Review new app</DialogTitle>
        <DialogContent dividers>
          <Stack spacing={2}>
            {createError && <OiErrorAlert error={createError} />}
            <Typography variant="body2" color="text.secondary">
              Creating app{" "}
              <Box component="span" sx={{ fontFamily: "monospace", fontWeight: 500 }}>
                {name}
              </Box>
              . The app will be registered in the <code>NotInstalled</code> state;
              no resources start until you run install.
            </Typography>
            {preview && <ScriptInventory preview={preview} />}
          </Stack>
        </DialogContent>
        <DialogActions>
          <Button onClick={handleCancel} disabled={creating}>
            Back to editor
          </Button>
          <Tooltip title={writeGuard.reason ?? ""}>
            <span>
              <Button
                variant="contained"
                onClick={handleConfirm}
                disabled={creating || preview?.script_error !== null || !writeGuard.allowed}
              >
                {creating ? (
                  <>
                    <CircularProgress size={14} sx={{ mr: 1 }} /> Creating…
                  </>
                ) : (
                  "Create app"
                )}
              </Button>
            </span>
          </Tooltip>
        </DialogActions>
      </Dialog>
    </Box>
  );
}
