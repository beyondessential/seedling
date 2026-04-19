import {
  Box,
  Button,
  Stack,
  TextField,
  Typography,
} from "@mui/material";
import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { ScriptEditor } from "../components/ScriptEditor";
import { useOiAction } from "../hooks/useOiAction";

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
  const { execute, loading, error } = useOiAction();
  const [name, setName] = useState("");
  const [script, setScript] = useState("");
  const [nameTouched, setNameTouched] = useState(false);

  const validationError = nameError(name);
  const canSubmit = name.length > 0 && validationError === null && !loading;

  const handleCreate = async () => {
    if (!canSubmit) return;
    try {
      await execute("/apps/create", { app: name, script });
      navigate(`/apps/${name}`);
    } catch {
      // displayed via error
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
        <Button size="small" component={Link} to="/" disabled={loading}>
          Cancel
        </Button>
        <Button
          size="small"
          variant="contained"
          onClick={handleCreate}
          disabled={!canSubmit}
        >
          {loading ? "Creating…" : "Create"}
        </Button>
      </Box>

      <Stack spacing={2}>
        {error && <OiErrorAlert error={error} />}

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
          onKeyDown={(e) => {
            if (e.key === "Enter" && canSubmit) void handleCreate();
          }}
        />

        <ScriptEditor value={script} onChange={setScript} />
      </Stack>
    </Box>
  );
}
