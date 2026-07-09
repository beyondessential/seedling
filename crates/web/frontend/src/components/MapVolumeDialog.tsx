import {
  Button,
  Checkbox,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  FormControl,
  FormControlLabel,
  FormLabel,
  InputLabel,
  MenuItem,
  Radio,
  RadioGroup,
  Select,
  Stack,
  TextField,
  Typography,
} from "@mui/material";
import { useState } from "react";
import { SolidActionButton } from "./ActionButton";
import { OiErrorAlert } from "./OiErrorAlert";
import { useOiAction } from "../hooks/useOiAction";
import { useOiQuery } from "../hooks/useOi";
import type {
  DeclaredExternalVolume,
  ExportedVolume,
  ExternalMapping,
  SiteVolume,
} from "../lib/types";

interface Props {
  open: boolean;
  onClose: () => void;
  onSuccess: () => void;
  /** Remap an existing mapping — app+name are fixed, only target is editable. */
  existing?: ExternalMapping;
  /** Pre-fill app+name for a new mapping (e.g. launched from app detail). */
  prefill?: { app: string; name: string };
}

export function MapVolumeDialog({ open, onClose, onSuccess, existing, prefill }: Props) {
  const { execute, loading, error, clearError } = useOiAction();

  const isRemap = !!existing;
  const isFixed = isRemap || !!prefill;

  const initialApp = existing?.app ?? prefill?.app ?? "";
  const initialName = existing?.external_name ?? prefill?.name ?? "";

  const [selectedRequest, setSelectedRequest] = useState<string>(
    initialApp && initialName ? `${initialApp}\0${initialName}` : "",
  );
  const [targetKind, setTargetKind] = useState<"site" | "app">(
    existing?.target.kind ?? "site",
  );
  const [targetApp, setTargetApp] = useState(
    existing?.target.kind === "app" ? existing.target.app : "",
  );
  const [targetVolume, setTargetVolume] = useState(
    existing
      ? existing.target.kind === "app"
        ? existing.target.volume
        : existing.target.name
      : "",
  );
  const [readOnly, setReadOnly] = useState(existing?.read_only ?? false);

  const { data: siteVolumes } = useOiQuery<SiteVolume[]>("/volumes/site/list", {});
  const { data: exportedVolumes } = useOiQuery<ExportedVolume[]>("/volumes/exported/list", {});
  const { data: declared } = useOiQuery<DeclaredExternalVolume[]>("/volumes/external/declared", {});

  const resolvedApp = isFixed ? initialApp : selectedRequest.split("\0")[0] ?? "";
  const resolvedName = isFixed ? initialName : selectedRequest.split("\0")[1] ?? "";

  const canSubmit =
    !!resolvedApp &&
    !!resolvedName &&
    !!targetVolume &&
    (targetKind === "site" || !!targetApp);

  const handleClose = () => {
    clearError();
    onClose();
  };

  const handleSubmit = async () => {
    const target =
      targetKind === "app"
        ? { kind: "app" as const, app: targetApp, volume: targetVolume }
        : { kind: "site" as const, name: targetVolume };
    const result = await execute(
      isRemap ? "/volumes/external/remap" : "/volumes/external/map",
      {
        app: resolvedApp,
        external_name: resolvedName,
        target,
        read_only: readOnly,
      },
    );
    if (result === null) return;
    onSuccess();
  };

  return (
    <Dialog open={open} onClose={handleClose} maxWidth="sm" fullWidth>
      <DialogTitle>
        {isRemap ? "Remap External Volume" : "Map External Volume"}
      </DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ mt: 0.5 }}>
          {error && <OiErrorAlert error={error} />}

          {isFixed ? (
            <TextField
              label="App / External volume"
              size="small"
              value={`${resolvedApp} / ${resolvedName}`}
              disabled
              slotProps={{
                htmlInput: { style: { fontFamily: "monospace" } }
              }}
            />
          ) : (
            <FormControl size="small" fullWidth>
              <InputLabel>App / External volume</InputLabel>
              <Select
                label="App / External volume"
                value={selectedRequest}
                onChange={(e) => {
                  setSelectedRequest(e.target.value);
                  setTargetVolume("");
                  setTargetApp("");
                }}
                sx={{ fontFamily: "monospace" }}
                autoFocus
              >
                {(declared ?? []).map((d) => (
                  <MenuItem
                    key={`${d.app}\0${d.name}`}
                    value={`${d.app}\0${d.name}`}
                    sx={{ fontFamily: "monospace" }}
                  >
                    {d.app}
                    <Typography
                      component="span"
                      sx={{
                        color: "text.secondary",
                        mx: 0.5
                      }}>/</Typography>
                    {d.name}
                  </MenuItem>
                ))}
              </Select>
            </FormControl>
          )}

          <FormControl>
            <FormLabel>Target</FormLabel>
            <RadioGroup
              row
              value={targetKind}
              onChange={(e) => {
                setTargetKind(e.target.value as typeof targetKind);
                setTargetVolume("");
              }}
            >
              <FormControlLabel value="site" control={<Radio size="small" />} label="Site volume" />
              <FormControlLabel value="app" control={<Radio size="small" />} label="Exported app volume" />
            </RadioGroup>
          </FormControl>

          {targetKind === "site" && (
            (siteVolumes ?? []).length > 0 ? (
              <FormControl size="small">
                <InputLabel>Site volume</InputLabel>
                <Select
                  label="Site volume"
                  value={targetVolume}
                  onChange={(e) => setTargetVolume(e.target.value)}
                  sx={{ fontFamily: "monospace" }}
                >
                  {(siteVolumes ?? []).map((v) => (
                    <MenuItem key={v.name} value={v.name} sx={{ fontFamily: "monospace" }}>
                      {v.name}
                      <Typography
                        component="span"
                        variant="caption"
                        sx={{
                          color: "text.secondary",
                          ml: 1
                        }}>
                        {v.kind}
                      </Typography>
                    </MenuItem>
                  ))}
                </Select>
              </FormControl>
            ) : (
              <TextField
                label="Site volume name"
                size="small"
                value={targetVolume}
                onChange={(e) => setTargetVolume(e.target.value)}
                helperText="No site volumes found — enter the name manually."
                slotProps={{
                  htmlInput: { style: { fontFamily: "monospace" } }
                }}
              />
            )
          )}

          {targetKind === "app" && (
            (exportedVolumes ?? []).length > 0 ? (
              <FormControl size="small" fullWidth>
                <InputLabel>Exported volume</InputLabel>
                <Select
                  label="Exported volume"
                  value={targetApp && targetVolume ? `${targetApp}\0${targetVolume}` : ""}
                  onChange={(e) => {
                    const parts = e.target.value.split("\0");
                    setTargetApp(parts[0] ?? "");
                    setTargetVolume(parts[1] ?? "");
                  }}
                  sx={{ fontFamily: "monospace" }}
                >
                  {(exportedVolumes ?? []).map((v) => (
                    <MenuItem
                      key={`${v.app}\0${v.volume_name}`}
                      value={`${v.app}\0${v.volume_name}`}
                      sx={{ fontFamily: "monospace" }}
                    >
                      {v.app}
                      <Typography
                        component="span"
                        sx={{
                          color: "text.secondary",
                          mx: 0.5
                        }}>/</Typography>
                      {v.volume_name}
                      {v.description && (
                        <Typography
                          component="span"
                          variant="caption"
                          sx={{
                            color: "text.secondary",
                            ml: 1
                          }}>
                          {v.description}
                        </Typography>
                      )}
                    </MenuItem>
                  ))}
                </Select>
              </FormControl>
            ) : (
              <>
                <TextField
                  label="Source app"
                  size="small"
                  value={targetApp}
                  onChange={(e) => {
                    setTargetApp(e.target.value);
                    setTargetVolume("");
                  }}
                  slotProps={{
                    htmlInput: { style: { fontFamily: "monospace" } }
                  }}
                />
                <TextField
                  label="Exported volume name"
                  size="small"
                  value={targetVolume}
                  onChange={(e) => setTargetVolume(e.target.value)}
                  slotProps={{
                    htmlInput: { style: { fontFamily: "monospace" } }
                  }}
                />
              </>
            )
          )}

          <FormControlLabel
            control={
              <Checkbox checked={readOnly} onChange={(e) => setReadOnly(e.target.checked)} size="small" />
            }
            label="Mount read-only"
          />
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={handleClose} disabled={loading}>Cancel</Button>
        <SolidActionButton
          safety="write"
          onClick={() => void handleSubmit()}
          disabled={loading || !canSubmit}
        >
          {loading
            ? isRemap ? "Remapping…" : "Mapping…"
            : isRemap ? "Remap" : "Map"}
        </SolidActionButton>
      </DialogActions>
    </Dialog>
  );
}
