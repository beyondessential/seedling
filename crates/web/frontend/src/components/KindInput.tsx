import {
  FormControl,
  FormHelperText,
  FormLabel,
  InputLabel,
  MenuItem,
  Select,
  TextField,
  ToggleButton,
  ToggleButtonGroup,
} from "@mui/material";
import { integerProblem } from "../lib/params";
import type { IntRange } from "../lib/types";

/** The parts of a param's schema its kind's input reads. */
export interface KindSchema {
  kind: string;
  required: boolean;
  description?: string | null;
  range?: IntRange | null;
  options?: string[] | null;
}

/** Whether the kind has an input of its own rather than a text field. */
export function hasKindInput(kind: string): boolean {
  return kind === "boolean" || kind === "integer" || kind === "choice";
}

/** The input for a `boolean`, `integer` or `choice` param. Every value is
 *  the kind's text, and the empty string is no value. */
// spec: ACN#presentation-of-kinds
export function KindInput({
  name,
  def,
  value,
  onChange,
  autoFocus,
}: {
  name: string;
  def: KindSchema;
  value: string;
  onChange: (value: string) => void;
  autoFocus?: boolean;
}) {
  const description = def.description || undefined;
  if (def.kind === "boolean") {
    return (
      <FormControl size="small" required={def.required}>
        <FormLabel id={`param-${name}-label`} sx={{ fontSize: "0.75rem" }}>
          {name}
        </FormLabel>
        <ToggleButtonGroup
          size="small"
          exclusive
          aria-labelledby={`param-${name}-label`}
          value={value === "" ? null : value}
          onChange={(_, next: string | null) => {
            // A param that is not required can be left without a value.
            if (next !== null || !def.required) onChange(next ?? "");
          }}
        >
          <ToggleButton value="true" autoFocus={autoFocus}>
            true
          </ToggleButton>
          <ToggleButton value="false">false</ToggleButton>
        </ToggleButtonGroup>
        {description && <FormHelperText>{description}</FormHelperText>}
      </FormControl>
    );
  }
  if (def.kind === "choice") {
    return (
      <FormControl size="small" required={def.required} fullWidth>
        <InputLabel id={`param-${name}-label`}>{name}</InputLabel>
        <Select
          labelId={`param-${name}-label`}
          label={name}
          value={value}
          autoFocus={autoFocus}
          onChange={(e) => onChange(e.target.value as string)}
        >
          {!def.required && (
            <MenuItem value="">
              <em>None</em>
            </MenuItem>
          )}
          {(def.options ?? []).map((option) => (
            <MenuItem key={option} value={option}>
              {option}
            </MenuItem>
          ))}
        </Select>
        {description && <FormHelperText>{description}</FormHelperText>}
      </FormControl>
    );
  }
  return (
    <TextField
      label={name}
      size="small"
      type="number"
      value={value}
      autoFocus={autoFocus}
      required={def.required}
      onChange={(e) => onChange(e.target.value)}
      helperText={integerProblem(value, def.range) ?? description}
      error={integerProblem(value, def.range) !== null}
      slotProps={{
        htmlInput: {
          step: 1,
          min: def.range?.min ?? undefined,
          max: def.range?.max ?? undefined,
          style: { fontFamily: "monospace" },
        },
      }}
    />
  );
}
