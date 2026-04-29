import AddIcon from "@mui/icons-material/Add";
import DeleteIcon from "@mui/icons-material/Delete";
import RefreshIcon from "@mui/icons-material/Refresh";
import {
  Box,
  Button,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Paper,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  TextField,
  Typography,
} from "@mui/material";
import { useCallback, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import {
  IconActionButton,
  SolidActionButton,
} from "../components/ActionButton";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { ScriptEditor } from "../components/ScriptEditor";
import { useEventRefresh } from "../hooks/useEventRefresh";
import { useOiQuery } from "../hooks/useOi";
import { useOiAction } from "../hooks/useOiAction";
import type { SeedlingEvent, TemplateSummary } from "../lib/types";

const NAME_RE = /^[a-zA-Z][a-zA-Z0-9-]{1,61}[a-zA-Z0-9]$/;

function nameError(name: string): string | null {
  if (name.length === 0) return null;
  if (name.length < 3) return "Name must be at least 3 characters.";
  if (name.length > 63) return "Name must be at most 63 characters.";
  if (!NAME_RE.test(name))
    return "Name must start with a letter, end with a letter or digit, and contain only letters, digits, or hyphens.";
  return null;
}

const TEMPLATE_EVENTS: Set<string> = new Set([
  "TemplateCreated",
  "TemplateUpdated",
  "TemplateRemoved",
  "TemplateInstantiated",
]);

export default function Templates() {
  const navigate = useNavigate();
  const {
    data: templates,
    loading,
    error,
    refetch,
  } = useOiQuery<TemplateSummary[]>("/templates/list", {});
  const { execute, loading: acting, error: actionError } = useOiAction();
  const { execute: removeExec, error: removeError } = useOiAction();

  const [dialogOpen, setDialogOpen] = useState(false);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [body, setBody] = useState("");
  const [nameTouched, setNameTouched] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);

  const matchTemplateEvents = useCallback(
    (ev: SeedlingEvent) => TEMPLATE_EVENTS.has(ev.type),
    [],
  );
  useEventRefresh(refetch, matchTemplateEvents);

  const validationError = nameError(name);
  const canSubmit =
    name.length > 0 && validationError === null && body.length > 0 && !acting;

  const resetForm = () => {
    setName("");
    setDescription("");
    setBody("");
    setNameTouched(false);
  };

  const handleCreate = async () => {
    if (!canSubmit) return;
    try {
      await execute("/templates/create", {
        name,
        body,
        description: description.trim() === "" ? null : description.trim(),
      });
      setDialogOpen(false);
      resetForm();
      refetch();
    } catch {
      // displayed via error
    }
  };

  const handleRemove = async (target: string) => {
    try {
      await removeExec("/templates/remove", { name: target });
      setConfirmRemove(null);
      refetch();
    } catch {
      // displayed via error
    }
  };

  return (
    <Box sx={{ p: 3, maxWidth: 900, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
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
        <Typography variant="body2" sx={{
          color: "text.disabled"
        }}>
          /
        </Typography>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          Templates
        </Typography>
        <SolidActionButton
          safety="write"
          size="small"
          startIcon={<AddIcon />}
          onClick={() => setDialogOpen(true)}
        >
          Upload template
        </SolidActionButton>
        <IconActionButton
          safety="read"
          tooltip="Refresh"
          onClick={refetch}
          disabled={loading}
        >
          <RefreshIcon />
        </IconActionButton>
      </Box>
      {error && <OiErrorAlert error={error} />}
      {removeError && <OiErrorAlert error={removeError} />}
      {loading && !templates && (
        <Box sx={{ display: "flex", justifyContent: "center", mt: 4 }}>
          <CircularProgress />
        </Box>
      )}
      {templates && (
        <TableContainer component={Paper} variant="outlined">
          <Table size="small">
            <TableHead>
              <TableRow>
                <TableCell>Name</TableCell>
                <TableCell>Description</TableCell>
                <TableCell>Created</TableCell>
                <TableCell width={40} />
              </TableRow>
            </TableHead>
            <TableBody>
              {templates.length === 0 && (
                <TableRow>
                  <TableCell
                    colSpan={4}
                    align="center"
                    sx={{ color: "text.secondary", py: 4 }}
                  >
                    No templates uploaded.
                  </TableCell>
                </TableRow>
              )}
              {templates.map((t) => (
                <TableRow
                  key={t.name}
                  hover
                  onClick={() => void navigate(`/templates/${t.name}`)}
                  sx={{ cursor: "pointer" }}
                >
                  <TableCell sx={{ fontFamily: "monospace", fontWeight: 500 }}>
                    {t.name}
                  </TableCell>
                  <TableCell sx={{ color: "text.secondary" }}>
                    {t.description ?? "—"}
                  </TableCell>
                  <TableCell sx={{ color: "text.secondary" }}>
                    {new Date(t.created_at).toLocaleString()}
                  </TableCell>
                  <TableCell align="right" sx={{ px: 0.5 }}>
                    <IconActionButton
                      safety="dangerous"
                      tooltip="Remove template"
                      onClick={(e) => {
                        e.stopPropagation();
                        setConfirmRemove(t.name);
                      }}
                    >
                      <DeleteIcon sx={{ fontSize: 16 }} />
                    </IconActionButton>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </TableContainer>
      )}
      <Dialog
        open={dialogOpen}
        onClose={() => {
          if (!acting) {
            setDialogOpen(false);
            resetForm();
          }
        }}
        maxWidth="lg"
        fullWidth
      >
        <DialogTitle>Upload template</DialogTitle>
        <DialogContent>
          <Stack spacing={2} sx={{ mt: 1 }}>
            {actionError && <OiErrorAlert error={actionError} />}
            <TextField
              label="Template name"
              size="small"
              value={name}
              onChange={(e) => setName(e.target.value)}
              onBlur={() => setNameTouched(true)}
              error={nameTouched && validationError !== null}
              helperText={nameTouched ? (validationError ?? " ") : " "}
              sx={{ maxWidth: 400 }}
              autoFocus
              slotProps={{
                htmlInput: { style: { fontFamily: "monospace" } }
              }}
            />
            <TextField
              label="Description (optional)"
              size="small"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              sx={{ maxWidth: 600 }}
            />
            <ScriptEditor value={body} onChange={setBody} minHeight="50vh" />
          </Stack>
        </DialogContent>
        <DialogActions>
          <Button
            onClick={() => {
              setDialogOpen(false);
              resetForm();
            }}
            disabled={acting}
          >
            Cancel
          </Button>
          <SolidActionButton
            safety="write"
            onClick={handleCreate}
            disabled={!canSubmit}
          >
            {acting ? "Uploading…" : "Upload"}
          </SolidActionButton>
        </DialogActions>
      </Dialog>
      <Dialog
        open={confirmRemove !== null}
        onClose={() => setConfirmRemove(null)}
      >
        <DialogTitle>Remove template?</DialogTitle>
        <DialogContent>
          <Typography>
            Remove template{" "}
            <Box
              component="span"
              sx={{ fontFamily: "monospace", fontWeight: 500 }}
            >
              {confirmRemove}
            </Box>
            ? Apps already instantiated from it are unaffected.
          </Typography>
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setConfirmRemove(null)}>Cancel</Button>
          <Button
            variant="contained"
            color="error"
            onClick={() => confirmRemove && void handleRemove(confirmRemove)}
          >
            Remove
          </Button>
        </DialogActions>
      </Dialog>
    </Box>
  );
}
