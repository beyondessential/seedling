import {
  Box,
  Button,
  CircularProgress,
  Stack,
  TextField,
  Tooltip,
  Typography,
} from "@mui/material";
import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useGuard } from "../components/SafetyModeProvider";
import { ScriptEditor } from "../components/ScriptEditor";
import { useOiAction } from "../hooks/useOiAction";
import { useOiQuery } from "../hooks/useOi";
import type { Template } from "../lib/types";

export default function EditTemplate() {
  const { name = "" } = useParams<{ name: string }>();
  const navigate = useNavigate();

  const {
    data,
    loading: fetching,
    error: fetchError,
  } = useOiQuery<Template>("/templates/show", { name });

  const { execute: saveExec, loading: saving, error: saveError } = useOiAction();
  const writeGuard = useGuard("write");

  const [body, setBody] = useState("");
  const [description, setDescription] = useState("");

  useEffect(() => {
    if (data) {
      setBody(data.body);
      setDescription(data.description ?? "");
    }
  }, [data]);

  const bodyChanged = data !== null && data?.body !== body;
  const descriptionChanged =
    data !== null && (data?.description ?? "") !== description;
  const unchanged = !bodyChanged && !descriptionChanged;
  const canSave = !saving && !!data && !unchanged;

  const handleSave = async () => {
    if (!canSave) return;
    const originalDescription = data?.description ?? null;
    const newDescription = description.trim() === "" ? null : description;
    const payload: Record<string, unknown> = { name };
    if (bodyChanged) payload.body = body;
    if (newDescription !== originalDescription) {
      payload.description = newDescription;
    }
    try {
      await saveExec("/templates/update", payload);
      navigate(`/templates/${name}`);
    } catch {
      // displayed via saveError
    }
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
        <Typography variant="body2" sx={{ color: "text.disabled" }}>
          /
        </Typography>
        <Typography
          component={Link}
          to={`/templates/${name}`}
          variant="body2"
          sx={{
            color: "text.secondary",
            textDecoration: "none",
            "&:hover": { textDecoration: "underline" },
          }}
        >
          {name}
        </Typography>
        <Typography variant="body2" sx={{ color: "text.disabled" }}>
          /
        </Typography>
        <Typography variant="body2">Edit</Typography>
        <Box sx={{ flexGrow: 1 }} />
        <Button
          size="small"
          onClick={() => navigate(`/templates/${name}`)}
          disabled={saving}
        >
          Cancel
        </Button>
        <Tooltip title={writeGuard.reason ?? ""}>
          <span>
            <Button
              size="small"
              variant="contained"
              onClick={handleSave}
              disabled={!canSave || !writeGuard.allowed}
            >
              {saving ? "Saving…" : unchanged ? "No changes" : "Save"}
            </Button>
          </span>
        </Tooltip>
      </Box>
      <Stack spacing={1}>
        {fetchError && <OiErrorAlert error={fetchError} />}
        {saveError && <OiErrorAlert error={saveError} />}
      </Stack>
      {fetching && !data && (
        <Box sx={{ display: "flex", justifyContent: "center", mt: 4 }}>
          <CircularProgress />
        </Box>
      )}
      {data && (
        <>
          <TextField
            label="Description"
            size="small"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            sx={{ maxWidth: 600 }}
          />
          <ScriptEditor value={body} onChange={setBody} minHeight="70vh" />
        </>
      )}
    </Box>
  );
}
