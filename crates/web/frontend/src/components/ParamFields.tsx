import CasinoIcon from "@mui/icons-material/Casino";
import VisibilityIcon from "@mui/icons-material/Visibility";
import VisibilityOffIcon from "@mui/icons-material/VisibilityOff";
import {
  Box,
  FormControl,
  FormHelperText,
  IconButton,
  InputAdornment,
  InputLabel,
  MenuItem,
  Select,
  TextField,
  Tooltip,
} from "@mui/material";
import { useState } from "react";
import { useOiQuery } from "../hooks/useOi";
import { generateRandomHex, paramFieldType, type ParamEntries } from "../lib/params";
import { isStrongPassword, passwordScore } from "../lib/passwordStrength";
import type { ParamSchema, SiteVolume } from "../lib/types";
import { OiErrorAlert } from "./OiErrorAlert";

function VolumeParamField({
  name,
  def,
  value,
  onChange,
}: {
  name: string;
  def: ParamSchema;
  value: string;
  onChange: (v: string) => void;
}) {
  const { data, error } = useOiQuery<SiteVolume[]>("/volumes/site/list", {});
  return (
    <FormControl size="small" required={def.required} fullWidth>
      <InputLabel id={`volume-${name}-label`}>{name}</InputLabel>
      <Select
        labelId={`volume-${name}-label`}
        label={name}
        value={value}
        onChange={(e) => onChange(e.target.value as string)}
      >
        {(data ?? []).map((v) => (
          <MenuItem key={v.name} value={v.name}>
            <Box sx={{ fontFamily: "monospace", display: "inline" }}>{v.name}</Box>
            <Box component="span" sx={{ ml: 1, color: "text.secondary" }}>
              {v.kind}
            </Box>
          </MenuItem>
        ))}
      </Select>
      {def.description && <FormHelperText>{def.description}</FormHelperText>}
      {error && <OiErrorAlert error={error} />}
    </FormControl>
  );
}

function helper(def: ParamSchema, value: string): string | undefined {
  if (def.kind === "password" && value.length > 0) {
    return isStrongPassword(value) ? (def.description || undefined) : "Password is too weak";
  }
  if (def.kind === "weak-password" && value.length > 0) {
    return `Strength: ${passwordScore(value)}/4${def.description ? ` — ${def.description}` : ""}`;
  }
  return def.description || undefined;
}

/** One input per declared param of an action or shell: each
 *  pre-filled by its owner with the param's `default_value`, required ones marked. */
// spec: ACN#param-schemas
// spec: ACN#presentation-of-kinds
export function ParamFields({
  entries,
  values,
  onChange,
}: {
  entries: ParamEntries;
  values: Record<string, string>;
  onChange: (key: string, value: string) => void;
}) {
  const [shown, setShown] = useState<Record<string, boolean>>({});

  return (
    <>
      {entries.map(([key, def]) => {
        const value = values[key] ?? "";
        if (def.kind === "volume") {
          return (
            <VolumeParamField
              key={key}
              name={key}
              def={def}
              value={value}
              onChange={(v) => onChange(key, v)}
            />
          );
        }
        const masked = def.secret === true || def.kind === "password" || def.kind === "weak-password";
        const isRandom = def.kind === "random";
        const isMultiline = def.kind === "multiline";
        const weak = def.kind === "password" && value.length > 0 && !isStrongPassword(value);
        return (
          <TextField
            key={key}
            label={key}
            size="small"
            value={value}
            onChange={(e) => onChange(key, e.target.value)}
            helperText={helper(def, value)}
            error={weak}
            multiline={isMultiline}
            minRows={isMultiline ? 3 : undefined}
            type={isMultiline ? undefined : shown[key] ? "text" : paramFieldType(def.kind, def.secret)}
            required={def.required}
            slotProps={{
              input:
                masked || isRandom
                  ? {
                      endAdornment: (
                        <InputAdornment position="end">
                          {isRandom && (
                            <Tooltip title="Generate (32 bytes, hex)">
                              <IconButton
                                size="small"
                                aria-label={`Generate ${key}`}
                                onClick={() => onChange(key, generateRandomHex())}
                              >
                                <CasinoIcon fontSize="small" />
                              </IconButton>
                            </Tooltip>
                          )}
                          {masked && (
                            <Tooltip title={shown[key] ? "Hide" : "Show"}>
                              <IconButton
                                size="small"
                                edge="end"
                                onClick={() => setShown((s) => ({ ...s, [key]: !s[key] }))}
                              >
                                {shown[key] ? (
                                  <VisibilityOffIcon fontSize="small" />
                                ) : (
                                  <VisibilityIcon fontSize="small" />
                                )}
                              </IconButton>
                            </Tooltip>
                          )}
                        </InputAdornment>
                      ),
                    }
                  : undefined,
              htmlInput: { style: { fontFamily: "monospace" } },
            }}
          />
        );
      })}
    </>
  );
}
