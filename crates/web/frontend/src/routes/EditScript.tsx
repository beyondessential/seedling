import {
  Box,
  Button,
  CircularProgress,
  Stack,
  Typography,
} from "@mui/material";
import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { ScriptEditor } from "../components/ScriptEditor";
import { useOiAction } from "../hooks/useOiAction";
import { useOiQuery } from "../hooks/useOi";

interface ScriptResponse {
  script: string;
  generation: number;
}

export default function EditScript() {
  const { name } = useParams<{ name: string }>();
  const navigate = useNavigate();

  const {
    data,
    loading: fetching,
    error: fetchError,
  } = useOiQuery<ScriptResponse>("/apps/script", { app: name });

  const { execute, loading: saving, error: saveError } = useOiAction();
  const [script, setScript] = useState("");

  useEffect(() => {
    if (data) setScript(data.script);
  }, [data]);

  const handleSave = async () => {
    try {
      await execute("/apps/update", { app: name, script });
      navigate(`/apps/${name}`);
    } catch {
      // displayed via saveError
    }
  };

  return (
    <Box sx={{ p: 3, maxWidth: 960, mx: "auto", display: "flex", flexDirection: "column", gap: 2 }}>
      <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
        <Typography
          component={Link}
          to="/"
          variant="body2"
          sx={{ color: "text.secondary", textDecoration: "none", "&:hover": { textDecoration: "underline" } }}
        >
          Apps
        </Typography>
        <Typography variant="body2" color="text.disabled">/</Typography>
        <Typography
          component={Link}
          to={`/apps/${name}`}
          variant="body2"
          sx={{ color: "text.secondary", textDecoration: "none", "&:hover": { textDecoration: "underline" } }}
        >
          {name}
        </Typography>
        <Typography variant="body2" color="text.disabled">/</Typography>
        <Typography variant="body2">Edit script</Typography>
        <Box sx={{ flexGrow: 1 }} />
        <Button
          size="small"
          onClick={() => navigate(`/apps/${name}`)}
          disabled={saving}
        >
          Cancel
        </Button>
        <Button
          size="small"
          variant="contained"
          onClick={handleSave}
          disabled={saving || fetching || !data}
        >
          {saving ? "Saving…" : "Save"}
        </Button>
      </Box>

      <Stack spacing={1}>
        {fetchError && <OiErrorAlert error={fetchError} />}
        {saveError && <OiErrorAlert error={saveError} />}
      </Stack>

      {fetching && (
        <Box sx={{ display: "flex", justifyContent: "center", mt: 4 }}>
          <CircularProgress />
        </Box>
      )}

      {data && (
        <ScriptEditor value={script} onChange={setScript} minHeight="70vh" />
      )}
    </Box>
  );
}
