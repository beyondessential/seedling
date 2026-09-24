import {
  Alert,
  Box,
  Button,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Stack,
  Typography,
} from "@mui/material";
import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { SolidActionButton } from "../components/ActionButton";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { PlanDiff } from "../components/PlanDiff";
import { ScriptEditor } from "../components/ScriptEditor";
import { useOiAction } from "../hooks/useOiAction";
import { useOiQuery } from "../hooks/useOi";
import type {
  AppBundleResponse,
  DiscoverResponse,
  ImagePin,
  ImageSummary,
  PlanResponse,
} from "../lib/types";

interface ScriptResponse {
  script: string;
  files?: string[];
  generation: number;
}

function utf8ToBase64(text: string): string {
  let binary = "";
  for (const byte of new TextEncoder().encode(text)) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}

export default function EditScript() {
  const { name } = useParams<{ name: string }>();
  const navigate = useNavigate();

  const {
    data,
    loading: fetching,
    error: fetchError,
  } = useOiQuery<ScriptResponse>("/apps/script", { app: name });
  const { data: bundle, error: bundleError } = useOiQuery<AppBundleResponse>(
    "/apps/bundle",
    { app: name },
  );

  const { execute: planExec, loading: planning, error: planError } = useOiAction();
  const { execute: discoverExec } = useOiAction();
  const { execute: saveExec, loading: saving, error: saveError } = useOiAction();
  const [script, setScript] = useState("");
  const [plan, setPlan] = useState<PlanResponse | null>(null);
  const [unwarmedHandlerImages, setUnwarmedHandlerImages] = useState<string[]>(
    [],
  );

  useEffect(() => {
    if (data) setScript(data.script);
  }, [data]);

  // w[impl routes.apps.definition.edit]
  // Only a definition with exactly one script file is edited here; every
  // other file in its bundle is carried over untouched.
  const scriptFiles = data?.files ?? ["app.seed.rhai"];
  const readOnly = scriptFiles.length > 1;
  const scriptFile = scriptFiles[0];
  // Until the bundle has arrived, what else the definition holds is
  // unknown: submitting a lone script then would replace the whole
  // definition with one file and take the sidecars with it.
  const files = bundle?.bundle ?? null;
  const hasSidecars =
    files !== null && Object.keys(files).some((p) => p !== scriptFile);
  /** The edited definition as request fields, prefixed for `/apps/plan`. */
  const definitionFields = (prefix: "" | "proposed_") =>
    files && hasSidecars
      ? {
          [`${prefix}bundle`]: { ...files, [scriptFile]: utf8ToBase64(script) },
        }
      : { [`${prefix}script`]: script };

  const unchanged = data !== null && data?.script === script;
  const canReview =
    !saving && !planning && !!data && !!files && !unchanged && !readOnly;

  const handleReview = async () => {
    if (!canReview) return;
    try {
      const result = (await planExec("/apps/plan", {
        app: name,
        ...definitionFields("proposed_"),
      })) as PlanResponse;
      setPlan(result);
      setUnwarmedHandlerImages([]);
      // Best-effort: if the plan had no errors, probe the proposed script
      // and cross-check its dynamic images against what's currently
      // present + pinned. Errors here are silently ignored — the user
      // can still proceed with the update.
      // w[impl routes.images.discover]
      if ((result.errors?.length ?? 0) === 0) {
        try {
          const [discovered, imgs, pins] = await Promise.all([
            discoverExec("/apps/images/discover", {
              app: name,
              proposed_script: script,
              lenient: true,
            }) as Promise<DiscoverResponse>,
            discoverExec("/images/list", {}) as Promise<{
              images: ImageSummary[];
            }>,
            discoverExec("/images/pins/list", { app: name }) as Promise<{
              pins: ImagePin[];
            }>,
          ]);
          const present = new Set<string>();
          for (const img of imgs.images) {
            for (const t of img.tags) present.add(t);
            for (const d of img.digests) present.add(d.reference);
          }
          for (const p of pins.pins) present.add(p.reference);
          setUnwarmedHandlerImages(
            discovered.all_images.filter((r) => !present.has(r)),
          );
        } catch {
          // silently swallow — this is advisory
        }
      }
    } catch {
      // displayed via planError
    }
  };

  const handleConfirm = async () => {
    try {
      await saveExec("/apps/update", { app: name, ...definitionFields("") });
      navigate(`/apps/${name}`);
    } catch {
      // displayed via saveError
    }
  };

  const handleCancel = () => {
    setPlan(null);
  };

  const planHasErrors =
    (plan?.errors?.length ?? 0) > 0 || (plan?.rejections?.length ?? 0) > 0;

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
        <Typography variant="body2" sx={{
          color: "text.disabled"
        }}>/</Typography>
        <Typography
          component={Link}
          to={`/apps/${name}`}
          variant="body2"
          sx={{ color: "text.secondary", textDecoration: "none", "&:hover": { textDecoration: "underline" } }}
        >
          {name}
        </Typography>
        <Typography variant="body2" sx={{
          color: "text.disabled"
        }}>/</Typography>
        <Typography variant="body2">Edit script</Typography>
        <Box sx={{ flexGrow: 1 }} />
        <Button
          size="small"
          onClick={() => navigate(`/apps/${name}`)}
          disabled={saving || planning}
        >
          Cancel
        </Button>
        <SolidActionButton
          safety="write"
          size="small"
          onClick={handleReview}
          disabled={!canReview}
        >
          {planning
            ? "Planning…"
            : unchanged
              ? "No changes"
              : !files
                ? "Loading…"
                : "Review & apply"}
        </SolidActionButton>
      </Box>
      <Stack spacing={1}>
        {fetchError && <OiErrorAlert error={fetchError} />}
        {bundleError && <OiErrorAlert error={bundleError} />}
        {planError && <OiErrorAlert error={planError} />}
      </Stack>
      {fetching && (
        <Box sx={{ display: "flex", justifyContent: "center", mt: 4 }}>
          <CircularProgress />
        </Box>
      )}
      {readOnly && (
        <Alert severity="info">
          This definition's script spans several files. Update it by pushing or
          fetching.
        </Alert>
      )}
      {data && (
        <ScriptEditor
          value={script}
          onChange={setScript}
          minHeight="70vh"
          readOnly={readOnly}
        />
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
            {plan && (
              <PlanDiff
                plan={plan}
                unwarmedHandlerImages={unwarmedHandlerImages}
              />
            )}
          </Stack>
        </DialogContent>
        <DialogActions>
          <Button onClick={handleCancel} disabled={saving}>
            Back to editor
          </Button>
          <SolidActionButton
            safety="write"
            onClick={handleConfirm}
            disabled={saving || planHasErrors}
          >
            {saving ? (
              <>
                <CircularProgress size={14} sx={{ mr: 1 }} /> Applying…
              </>
            ) : (
              "Apply"
            )}
          </SolidActionButton>
        </DialogActions>
      </Dialog>
    </Box>
  );
}
