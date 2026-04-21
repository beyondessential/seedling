import {
  Box,
  Button,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Stack,
  Tooltip,
  Typography,
} from "@mui/material";
import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { PlanDiff } from "../components/PlanDiff";
import { useGuard } from "../components/SafetyModeProvider";
import { ScriptEditor } from "../components/ScriptEditor";
import { useOiAction } from "../hooks/useOiAction";
import { useOiQuery } from "../hooks/useOi";
import type { PlanResponse } from "../lib/types";

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

  const { execute: planExec, loading: planning, error: planError } = useOiAction();
  const { execute: saveExec, loading: saving, error: saveError } = useOiAction();
  const writeGuard = useGuard("write");
  const [script, setScript] = useState("");
  const [plan, setPlan] = useState<PlanResponse | null>(null);

  useEffect(() => {
    if (data) setScript(data.script);
  }, [data]);

  const unchanged = data !== null && data?.script === script;
  const canReview = !saving && !planning && !!data && !unchanged;

  const handleReview = async () => {
    if (!canReview) return;
    try {
      const result = (await planExec("/apps/plan", {
        app: name,
        proposed_script: script,
      })) as PlanResponse;
      setPlan(result);
    } catch {
      // displayed via planError
    }
  };

  const handleConfirm = async () => {
    try {
      await saveExec("/apps/update", { app: name, script });
      navigate(`/apps/${name}`);
    } catch {
      // displayed via saveError
    }
  };

  const handleCancel = () => {
    setPlan(null);
  };

  const planHasErrors = (plan?.errors?.length ?? 0) > 0;

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
          disabled={saving || planning}
        >
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
              {planning ? "Planning…" : unchanged ? "No changes" : "Review & apply"}
            </Button>
          </span>
        </Tooltip>
      </Box>

      <Stack spacing={1}>
        {fetchError && <OiErrorAlert error={fetchError} />}
        {planError && <OiErrorAlert error={planError} />}
      </Stack>

      {fetching && (
        <Box sx={{ display: "flex", justifyContent: "center", mt: 4 }}>
          <CircularProgress />
        </Box>
      )}

      {data && (
        <ScriptEditor value={script} onChange={setScript} minHeight="70vh" />
      )}

      <Dialog
        open={plan !== null}
        onClose={() => !saving && handleCancel()}
        maxWidth="md"
        fullWidth
      >
        <DialogTitle>
          Review changes ·{" "}
          <Box component="span" sx={{ fontFamily: "monospace" }}>
            {name}
          </Box>
        </DialogTitle>
        <DialogContent dividers>
          <Stack spacing={2}>
            {saveError && <OiErrorAlert error={saveError} />}
            {plan && <PlanDiff plan={plan} />}
          </Stack>
        </DialogContent>
        <DialogActions>
          <Button onClick={handleCancel} disabled={saving}>
            Back to editor
          </Button>
          <Tooltip title={writeGuard.reason ?? ""}>
            <span>
              <Button
                variant="contained"
                onClick={handleConfirm}
                disabled={saving || planHasErrors || !writeGuard.allowed}
              >
                {saving ? (
                  <>
                    <CircularProgress size={14} sx={{ mr: 1 }} /> Applying…
                  </>
                ) : (
                  "Apply"
                )}
              </Button>
            </span>
          </Tooltip>
        </DialogActions>
      </Dialog>
    </Box>
  );
}
