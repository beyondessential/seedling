import DeleteIcon from "@mui/icons-material/Delete";
import PlayArrowIcon from "@mui/icons-material/PlayArrow";
import {
  Alert,
  Box,
  Button,
  Chip,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Divider,
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
import CodeMirror from "@uiw/react-codemirror";
import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useOiAction } from "../hooks/useOiAction";
import { useOiQuery } from "../hooks/useOi";
import { rhaiLanguage } from "../lib/rhai-lang";
import type { Template, TemplatePreview } from "../lib/types";

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
  } = useOiQuery<Template>("/templates/show", { name });
  const { data: preview, error: previewError } = useOiQuery<TemplatePreview>(
    "/templates/preview",
    { name },
  );
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
        <Typography variant="body2" color="text.disabled">
          /
        </Typography>
        <Typography variant="h5" sx={{ flexGrow: 1, fontFamily: "monospace" }}>
          {template.name}
        </Typography>
        <Button
          size="small"
          variant="contained"
          startIcon={<PlayArrowIcon />}
          onClick={() => setInstantiateOpen(true)}
        >
          Create app from template
        </Button>
        <Button
          size="small"
          color="error"
          startIcon={<DeleteIcon />}
          onClick={() => setConfirmRemove(true)}
        >
          Remove
        </Button>
      </Box>

      {removeError && <OiErrorAlert error={removeError} />}

      <Stack spacing={3}>
        {template.description && (
          <Typography variant="body2" color="text.secondary">
            {template.description}
          </Typography>
        )}

        <Typography variant="caption" color="text.secondary">
          Uploaded {new Date(template.created_at).toLocaleString()}
        </Typography>

        <Divider />

        <Box>
          <Typography variant="subtitle1" sx={{ mb: 1, fontWeight: 600 }}>
            Preview
          </Typography>
          {previewError && <OiErrorAlert error={previewError} />}
          {preview?.script_error && (
            <Alert severity="error" sx={{ mb: 2, fontFamily: "monospace" }}>
              {preview.script_error}
            </Alert>
          )}
          {preview && (
            <Stack spacing={2}>
              <PreviewSection
                title="Resources"
                count={preview.resources.length}
              >
                {preview.resources.length > 0 && (
                  <TableContainer component={Paper} variant="outlined">
                    <Table size="small">
                      <TableHead>
                        <TableRow>
                          <TableCell>Name</TableCell>
                          <TableCell>Type</TableCell>
                          <TableCell>Summary</TableCell>
                        </TableRow>
                      </TableHead>
                      <TableBody>
                        {preview.resources.map((r) => (
                          <TableRow key={`${r.type}/${r.name}`}>
                            <TableCell sx={{ fontFamily: "monospace" }}>
                              {r.name}
                            </TableCell>
                            <TableCell>
                              <Chip
                                label={r.type}
                                size="small"
                                variant="outlined"
                              />
                            </TableCell>
                            <TableCell
                              sx={{
                                fontFamily: "monospace",
                                color: "text.secondary",
                                fontSize: "0.75rem",
                              }}
                            >
                              {resourceSummary(r)}
                            </TableCell>
                          </TableRow>
                        ))}
                      </TableBody>
                    </Table>
                  </TableContainer>
                )}
              </PreviewSection>

              <PreviewSection title="Params" count={preview.params.length}>
                {preview.params.length > 0 && (
                  <TableContainer component={Paper} variant="outlined">
                    <Table size="small">
                      <TableHead>
                        <TableRow>
                          <TableCell>Name</TableCell>
                          <TableCell>Kind</TableCell>
                          <TableCell>Required</TableCell>
                          <TableCell>Default</TableCell>
                          <TableCell>Description</TableCell>
                        </TableRow>
                      </TableHead>
                      <TableBody>
                        {preview.params.map((p) => (
                          <TableRow key={p.name}>
                            <TableCell sx={{ fontFamily: "monospace" }}>
                              {p.name}
                              {p.secret && (
                                <Chip
                                  label="secret"
                                  size="small"
                                  sx={{ ml: 1 }}
                                  variant="outlined"
                                />
                              )}
                            </TableCell>
                            <TableCell>{p.kind}</TableCell>
                            <TableCell>{p.required ? "yes" : "no"}</TableCell>
                            <TableCell
                              sx={{
                                fontFamily: "monospace",
                                color: "text.secondary",
                              }}
                            >
                              {p.default_value ?? "—"}
                            </TableCell>
                            <TableCell sx={{ color: "text.secondary" }}>
                              {p.description ?? "—"}
                            </TableCell>
                          </TableRow>
                        ))}
                      </TableBody>
                    </Table>
                  </TableContainer>
                )}
              </PreviewSection>

              <PreviewSection title="Actions" count={preview.actions.length}>
                {preview.actions.length > 0 && (
                  <TableContainer component={Paper} variant="outlined">
                    <Table size="small">
                      <TableHead>
                        <TableRow>
                          <TableCell>Name</TableCell>
                          <TableCell>Kind</TableCell>
                          <TableCell>Description</TableCell>
                        </TableRow>
                      </TableHead>
                      <TableBody>
                        {preview.actions.map((a) => (
                          <TableRow key={`${a.kind}/${a.name}`}>
                            <TableCell sx={{ fontFamily: "monospace" }}>
                              {a.name}
                            </TableCell>
                            <TableCell>
                              <Chip
                                label={a.kind}
                                size="small"
                                variant="outlined"
                              />
                            </TableCell>
                            <TableCell sx={{ color: "text.secondary" }}>
                              {a.description ?? "—"}
                            </TableCell>
                          </TableRow>
                        ))}
                      </TableBody>
                    </Table>
                  </TableContainer>
                )}
              </PreviewSection>
            </Stack>
          )}
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
            <Typography variant="body2" color="text.secondary">
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
              inputProps={{ style: { fontFamily: "monospace" } }}
              autoFocus
              onKeyDown={(e) => {
                if (e.key === "Enter" && canInstantiate) void handleInstantiate();
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
          <Button
            variant="contained"
            onClick={handleInstantiate}
            disabled={!canInstantiate}
          >
            {acting ? "Creating…" : "Create app"}
          </Button>
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
            color="error"
            variant="contained"
            onClick={() => void handleRemove()}
          >
            Remove
          </Button>
        </DialogActions>
      </Dialog>
    </Box>
  );
}

function PreviewSection({
  title,
  count,
  children,
}: {
  title: string;
  count: number;
  children: React.ReactNode;
}) {
  return (
    <Box>
      <Typography
        variant="caption"
        color="text.secondary"
        sx={{ display: "block", mb: 0.5 }}
      >
        {title} ({count})
      </Typography>
      {count === 0 ? (
        <Typography
          variant="body2"
          color="text.disabled"
          sx={{ fontStyle: "italic" }}
        >
          None declared.
        </Typography>
      ) : (
        children
      )}
    </Box>
  );
}

function resourceSummary(r: {
  type: string;
  def?: unknown;
  scale?: { low: number; high: number };
}): string {
  const parts: string[] = [];
  if (r.scale) parts.push(`scale ${r.scale.low}..${r.scale.high}`);
  const def = r.def as Record<string, unknown> | undefined;
  if (def) {
    if (typeof def.image === "string") parts.push(def.image);
    const container = def.container as Record<string, unknown> | undefined;
    if (container && typeof container.image === "string") {
      parts.push(container.image as string);
    }
    if (typeof def.hostname === "string") parts.push(def.hostname as string);
    if (typeof def.service === "string" && typeof def.port === "number") {
      parts.push(`${def.service as string}:${def.port as number}`);
    }
  }
  return parts.join(" · ");
}
