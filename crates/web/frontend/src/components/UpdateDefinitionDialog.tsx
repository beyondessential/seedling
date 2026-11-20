import AddIcon from "@mui/icons-material/Add";
import ClearIcon from "@mui/icons-material/Clear";
import {
  Autocomplete,
  Box,
  Button,
  Checkbox,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  FormControlLabel,
  IconButton,
  Stack,
  TextField,
  Tooltip,
} from "@mui/material";
import { useEffect, useState } from "react";
import { useOiAction } from "../hooks/useOiAction";
import type { ParamChanges, PlanResponse } from "../lib/types";
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

/** A parameter change being drafted: a value to set, or an unset. */
interface ParamDraft {
  name: string;
  value: string;
  unset: boolean;
}

/** The change the drafts make, or why they cannot be sent: rows with no name
 *  are left out, and a name given twice would be one change made twice. */
export function paramChangesOf(
  drafts: ParamDraft[],
): { changes: ParamChanges | null; duplicate: string | null } {
  const changes: ParamChanges = {};
  for (const d of drafts) {
    const name = d.name.trim();
    if (!name) continue;
    if (name in changes) return { changes: null, duplicate: name };
    changes[name] = d.unset ? null : d.value;
  }
  return { changes: Object.keys(changes).length > 0 ? changes : null, duplicate: null };
}

// spec: UI#app-definitions
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
  const [drafts, setDrafts] = useState<ParamDraft[]>([]);
  const [plan, setPlan] = useState<PlanResponse | null>(null);

  useEffect(() => {
    if (!repository) return;
    void tagsExec("/registries/tags", { repository }).then((r) => {
      if (r) setTags((r as { tags: string[] }).tags);
    });
  }, [repository, tagsExec]);

  const reference = referenceFor(input, repository);
  const { changes, duplicate } = paramChangesOf(drafts);

  // Any edit invalidates the review: what is applied must be what was planned.
  const edit = <T,>(set: (v: T) => void) => (v: T) => {
    set(v);
    setPlan(null);
  };

  const changeDraft = (index: number, over: Partial<ParamDraft>) =>
    edit(setDrafts)(drafts.map((d, i) => (i === index ? { ...d, ...over } : d)));

  const handleReview = async () => {
    if (!reference || duplicate) return;
    const result = await planExec("/apps/plan", {
      app: appName,
      proposed_reference: reference,
      ...(changes ? { proposed_params: changes } : {}),
    });
    if (result) setPlan(result as PlanResponse);
  };

  const handleApply = async () => {
    if (!reference) return;
    const result = await saveExec("/apps/update", {
      app: appName,
      reference,
      ...(changes ? { params: changes } : {}),
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
          </Box>
          <Stack spacing={1}>
            {drafts.map((d, i) => (
              <Box key={i} sx={{ display: "flex", gap: 1, alignItems: "flex-start" }}>
                <TextField
                  label="Param"
                  size="small"
                  value={d.name}
                  onChange={(e) => changeDraft(i, { name: e.target.value })}
                  error={duplicate !== null && d.name.trim() === duplicate}
                  helperText={
                    duplicate !== null && d.name.trim() === duplicate
                      ? "Named more than once"
                      : undefined
                  }
                />
                <TextField
                  label="Value"
                  size="small"
                  value={d.value}
                  disabled={d.unset || !d.name.trim()}
                  onChange={(e) => changeDraft(i, { value: e.target.value })}
                />
                <FormControlLabel
                  control={
                    <Checkbox
                      size="small"
                      checked={d.unset}
                      onChange={(e) => changeDraft(i, { unset: e.target.checked })}
                    />
                  }
                  label="Unset"
                />
                <Tooltip title="Remove this change">
                  <IconButton
                    size="small"
                    aria-label="Remove this change"
                    onClick={() => edit(setDrafts)(drafts.filter((_, j) => j !== i))}
                  >
                    <ClearIcon fontSize="small" />
                  </IconButton>
                </Tooltip>
              </Box>
            ))}
            <Box>
              <Button
                size="small"
                startIcon={<AddIcon fontSize="small" />}
                onClick={() => edit(setDrafts)([...drafts, { name: "", value: "", unset: false }])}
              >
                Change a param
              </Button>
              {drafts.length === 0 && (
                <Box component="span" sx={{ ml: 1, color: "text.secondary", fontSize: 12 }}>
                  Optional. Changes are applied with the new definition.
                </Box>
              )}
            </Box>
          </Stack>
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
          disabled={!reference || duplicate !== null || planning || saving}
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
