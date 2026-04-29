import DeleteIcon from "@mui/icons-material/Delete";
import EditIcon from "@mui/icons-material/Edit";
import PlayArrowIcon from "@mui/icons-material/PlayArrow";
import {
  Box,
  Button,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Divider,
  Stack,
  TextField,
  Typography,
} from "@mui/material";
import CodeMirror from "@uiw/react-codemirror";
import { useCallback, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  OutlinedActionButton,
  SolidActionButton,
} from "../components/ActionButton";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { ScriptInventory } from "../components/ScriptInventory";
import { useEventRefresh } from "../hooks/useEventRefresh";
import { useOiAction } from "../hooks/useOiAction";
import { useOiQuery } from "../hooks/useOi";
import { rhaiLanguage } from "../lib/rhai-lang";
import type { SeedlingEvent, Template, TemplatePreview } from "../lib/types";

const APP_NAME_RE = /^[a-zA-Z][a-zA-Z0-9-]{1,61}[a-zA-Z0-9]$/;

function appNameError(name: string): string | null {
  if (name.length === 0) return null;
  if (name.length < 3) return "Name must be at least 3 characters.";
  if (name.length > 63) return "Name must be at most 63 characters.";
  if (!APP_NAME_RE.test(name))
    return "Name must start with a letter, end with a letter or digit, and contain only letters, digits, or hyphens.";
  return null;
}

export default function TemplateDetail() {
  const { name = "" } = useParams<{ name: string }>();
  const navigate = useNavigate();
  const {
    data: template,
    loading,
    error,
    refetch,
  } = useOiQuery<Template>("/templates/show", { name });
  const {
    data: preview,
    error: previewError,
    refetch: refetchPreview,
  } = useOiQuery<TemplatePreview>("/templates/preview", { name });
  const matchThisTemplate = useCallback(
    (ev: SeedlingEvent) =>
      (ev.type === "TemplateUpdated" || ev.type === "TemplateRemoved") &&
      ev.name === name,
    [name],
  );
  useEventRefresh(() => {
    refetch();
    refetchPreview();
  }, matchThisTemplate);
  const { execute, loading: acting, error: actionError } = useOiAction();
  const { execute: removeExec, error: removeError } = useOiAction();

  const [instantiateOpen, setInstantiateOpen] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [appName, setAppName] = useState("");
  const [appNameTouched, setAppNameTouched] = useState(false);

  const appNameValidation = appNameError(appName);
  const canInstantiate =
    appName.length > 0 && appNameValidation === null && !acting;

  const handleInstantiate = async () => {
    if (!canInstantiate) return;
    try {
      await execute("/templates/instantiate", {
        template: name,
        app: appName,
      });
      setInstantiateOpen(false);
      navigate(`/apps/${appName}`);
    } catch {
      // displayed via error
    }
  };

  const handleRemove = async () => {
    try {
      await removeExec("/templates/remove", { name });
      setConfirmRemove(false);
      navigate("/templates");
    } catch {
      // displayed via error
    }
  };

  if (loading && !template) {
    return (
      <Box sx={{ display: "flex", justifyContent: "center", mt: 4 }}>
        <CircularProgress />
      </Box>
    );
  }

  if (error) {
    return (
      <Box sx={{ p: 3, maxWidth: 960, mx: "auto" }}>
        <OiErrorAlert error={error} />
      </Box>
    );
  }

  if (!template) return null;

  return (
    <Box sx={{ p: 3, maxWidth: 960, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", gap: 1, mb: 2 }}>
        <Typography
          component={Link}
          to="/templates"
          variant="body2"
          sx={{
            color: "text.secondary",
            textDecoration: "none",
            "&:hover": { textDecoration: "underline" },
          }}
        >
          Templates
        </Typography>
        <Typography variant="body2" sx={{
          color: "text.disabled"
        }}>
          /
        </Typography>
        <Typography variant="h5" sx={{ flexGrow: 1, fontFamily: "monospace" }}>
          {template.name}
        </Typography>
        <SolidActionButton
          safety="write"
          size="small"
          startIcon={<PlayArrowIcon />}
          onClick={() => setInstantiateOpen(true)}
        >
          Create app from template
        </SolidActionButton>
        <OutlinedActionButton
          safety="write"
          size="small"
          startIcon={<EditIcon />}
          onClick={() => navigate(`/templates/${template.name}/edit`)}
        >
          Edit
        </OutlinedActionButton>
        <OutlinedActionButton
          safety="dangerous"
          size="small"
          startIcon={<DeleteIcon />}
          onClick={() => setConfirmRemove(true)}
        >
          Remove
        </OutlinedActionButton>
      </Box>
      {removeError && <OiErrorAlert error={removeError} />}
      <Stack spacing={3}>
        {template.description && (
          <Typography variant="body2" sx={{
            color: "text.secondary"
          }}>
            {template.description}
          </Typography>
        )}

        <Typography variant="caption" sx={{
          color: "text.secondary"
        }}>
          Uploaded {new Date(template.created_at).toLocaleString()}
        </Typography>

        <Divider />

        <Box>
          <Typography variant="subtitle1" sx={{ mb: 1, fontWeight: 600 }}>
            Preview
          </Typography>
          {previewError && <OiErrorAlert error={previewError} />}
          {preview && <ScriptInventory preview={preview} />}
        </Box>

        <Divider />

        <Box>
          <Typography variant="subtitle1" sx={{ mb: 1, fontWeight: 600 }}>
            Script
          </Typography>
          <Box
            sx={{
              border: "1px solid",
              borderColor: "divider",
              borderRadius: 1,
              overflow: "hidden",
              "& .cm-scroller": {
                fontFamily: "monospace",
                fontSize: "0.875rem",
              },
            }}
          >
            <CodeMirror
              value={template.body}
              extensions={[rhaiLanguage]}
              editable={false}
              basicSetup={{
                lineNumbers: true,
                foldGutter: true,
                highlightActiveLine: false,
              }}
            />
          </Box>
        </Box>
      </Stack>
      <Dialog
        open={instantiateOpen}
        onClose={() => !acting && setInstantiateOpen(false)}
      >
        <DialogTitle>Create app from template</DialogTitle>
        <DialogContent>
          <Stack spacing={2} sx={{ mt: 1, minWidth: 360 }}>
            {actionError && <OiErrorAlert error={actionError} />}
            <Typography variant="body2" sx={{
              color: "text.secondary"
            }}>
              The template's script will be copied into a new app with the
              name below. The app is independent of the template.
            </Typography>
            <TextField
              label="New app name"
              size="small"
              value={appName}
              onChange={(e) => setAppName(e.target.value)}
              onBlur={() => setAppNameTouched(true)}
              error={appNameTouched && appNameValidation !== null}
              helperText={
                appNameTouched ? (appNameValidation ?? " ") : " "
              }
              autoFocus
              onKeyDown={(e) => {
                if (e.key === "Enter" && canInstantiate) void handleInstantiate();
              }}
              slotProps={{
                htmlInput: { style: { fontFamily: "monospace" } }
              }}
            />
          </Stack>
        </DialogContent>
        <DialogActions>
          <Button
            onClick={() => setInstantiateOpen(false)}
            disabled={acting}
          >
            Cancel
          </Button>
          <SolidActionButton
            safety="write"
            onClick={handleInstantiate}
            disabled={!canInstantiate}
          >
            {acting ? "Creating…" : "Create app"}
          </SolidActionButton>
        </DialogActions>
      </Dialog>
      <Dialog
        open={confirmRemove}
        onClose={() => setConfirmRemove(false)}
      >
        <DialogTitle>Remove template?</DialogTitle>
        <DialogContent>
          <Typography>
            Remove template{" "}
            <Box
              component="span"
              sx={{ fontFamily: "monospace", fontWeight: 500 }}
            >
              {template.name}
            </Box>
            ? Apps already instantiated from it are unaffected.
          </Typography>
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setConfirmRemove(false)}>Cancel</Button>
          <Button
            variant="contained"
            color="error"
            onClick={() => void handleRemove()}
          >
            Remove
          </Button>
        </DialogActions>
      </Dialog>
    </Box>
  );
}

