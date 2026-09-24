import {
  Autocomplete,
  Box,
  Button,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Stack,
  TextField,
} from "@mui/material";
import { useEffect, useState } from "react";
import { useOiAction } from "../hooks/useOiAction";
import type { PlanResponse } from "../lib/types";
import { OutlinedActionButton, SolidActionButton } from "./ActionButton";
import { OiErrorAlert } from "./OiErrorAlert";
import { PlanDiff } from "./PlanDiff";

/** Split a reference into its repository and its tag, if it names one. */
export function splitReference(reference: string): {
  repository: string;
  tag: string | null;
} {
  const name = reference.split("@")[0];
  const slash = name.lastIndexOf("/");
  const colon = name.lastIndexOf(":");
  if (colon > slash) {
    return { repository: name.slice(0, colon), tag: name.slice(colon + 1) };
  }
  return { repository: name, tag: null };
}

/** A tag picked from the list, or a complete reference typed in. */
export function referenceFor(input: string, repository: string | null): string | null {
  const trimmed = input.trim();
  if (!trimmed) return null;
  if (trimmed.includes("/")) return trimmed;
  return repository ? `${repository}:${trimmed}` : null;
}

// w[impl routes.apps.definition.fetch]
export function UpdateDefinitionDialog({
  appName,
  recordedReference,
  preselect,
  onClose,
  onApplied,
}: {
  appName: string;
  /** The reference the current definition was fetched from, if it was. */
  recordedReference: string | null;
  /** A reference to start from, such as a tag that has moved. */
  preselect: string | null;
  onClose: () => void;
  onApplied: () => void;
}) {
  const recorded = recordedReference ? splitReference(recordedReference) : null;
  const repository = recorded?.repository ?? null;
  const { execute: tagsExec, loading: loadingTags, error: tagsError } = useOiAction();
  const { execute: planExec, loading: planning, error: planError } = useOiAction();
  const { execute: saveExec, loading: saving, error: saveError } = useOiAction();
  const [tags, setTags] = useState<string[]>([]);
  const [input, setInput] = useState(
    preselect ? (splitReference(preselect).tag ?? preselect) : "",
  );
  const [paramName, setParamName] = useState("");
  const [paramValue, setParamValue] = useState("");
  const [plan, setPlan] = useState<PlanResponse | null>(null);

  useEffect(() => {
    if (!repository) return;
    void tagsExec("/registries/tags", { repository }).then((r) => {
      if (r) setTags((r as { tags: string[] }).tags);
    });
  }, [repository, tagsExec]);

  const reference = referenceFor(input, repository);
  const param = paramName.trim()
    ? { name: paramName.trim(), value: paramValue }
    : null;

  // Any edit invalidates the review: what is applied must be what was planned.
  const edit = <T,>(set: (v: T) => void) => (v: T) => {
    set(v);
    setPlan(null);
  };

  const handleReview = async () => {
    if (!reference) return;
    const result = await planExec("/apps/plan", {
      app: appName,
      proposed_reference: reference,
      ...(param ? { proposed_params: [param] } : {}),
    });
    if (result) setPlan(result as PlanResponse);
  };

  const handleApply = async () => {
    if (!reference) return;
    const result = await saveExec("/apps/update", {
      app: appName,
      reference,
      ...(param ? { param } : {}),
    });
    if (result !== null) onApplied();
  };

  const rejected =
    (plan?.errors?.length ?? 0) > 0 || (plan?.rejections?.length ?? 0) > 0;

  return (
    <Dialog open onClose={() => !saving && onClose()} maxWidth="md" fullWidth>
      <DialogTitle>
        Update definition ·{" "}
        <Box component="span" sx={{ fontFamily: "monospace" }}>
          {appName}
        </Box>
      </DialogTitle>
      <DialogContent dividers>
        <Stack spacing={2}>
          {tagsError && <OiErrorAlert error={tagsError} />}
          <Box sx={{ display: "flex", gap: 2 }}>
            <Autocomplete
              freeSolo
              sx={{ flex: 1 }}
              options={tags}
              loading={loadingTags}
              inputValue={input}
              onInputChange={(_, v) => edit(setInput)(v)}
              renderOption={(props, option) => (
                <li {...props} key={option}>
                  <Box component="span" sx={{ fontFamily: "monospace", flexGrow: 1 }}>
                    {option}
                  </Box>
                  {option === recorded?.tag && (
                    <Box component="span" sx={{ color: "text.secondary", fontSize: 12 }}>
                      current
                    </Box>
                  )}
                </li>
              )}
              renderInput={(params) => (
                <TextField
                  {...params}
                  label={repository ? "Tag or reference" : "Reference"}
                  size="small"
                  helperText={repository ?? "A complete reference, with registry and tag"}
                  slotProps={{
                    ...params.slotProps,
                    formHelperText: { sx: { fontFamily: "monospace" } },
                  }}
                />
              )}
            />
            <Box sx={{ flex: 1, display: "flex", gap: 1, alignItems: "flex-start" }}>
              <TextField
                label="Param"
                size="small"
                value={paramName}
                onChange={(e) => edit(setParamName)(e.target.value)}
                helperText="Optional. Applied with the new definition."
              />
              <TextField
                label="Value"
                size="small"
                value={paramValue}
                disabled={!paramName.trim()}
                onChange={(e) => edit(setParamValue)(e.target.value)}
              />
            </Box>
          </Box>
          {planError && <OiErrorAlert error={planError} />}
          {saveError && <OiErrorAlert error={saveError} />}
          {plan && <PlanDiff plan={plan} />}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose} disabled={saving}>
          Cancel
        </Button>
        <OutlinedActionButton
          safety="read"
          onClick={handleReview}
          disabled={!reference || planning || saving}
        >
          {planning ? "Planning…" : "Review"}
        </OutlinedActionButton>
        <SolidActionButton
          safety="write"
          onClick={handleApply}
          disabled={!plan || rejected || saving}
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
  );
}
