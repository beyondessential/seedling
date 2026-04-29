import {
  Box,
  Button,
  IconButton,
  Tooltip,
  type ButtonProps,
  type IconButtonProps,
} from "@mui/material";
import { alpha, type Theme } from "@mui/material/styles";
import { type ReactNode } from "react";
import { type SafetyMode, useGuard } from "./SafetyModeProvider";

interface CommonProps {
  /** Tier required to invoke. "read" buttons are always allowed; "write" and
   *  "dangerous" disable themselves outside the matching safety mode and on
   *  hover paint a tier-coloured diagonal-stripe background to make the
   *  requirement visible at a glance. */
  safety: SafetyMode;
  /** Tooltip shown on hover. Rendered verbatim — no tier prefix. */
  tooltip?: ReactNode;
  /** Disabled for reasons unrelated to safety (loading, invalid form, etc). */
  disabled?: boolean;
}

interface TextProps extends CommonProps {
  onClick?: ButtonProps["onClick"];
  startIcon?: ReactNode;
  color?: ButtonProps["color"];
  size?: ButtonProps["size"];
  type?: ButtonProps["type"];
  fullWidth?: boolean;
  sx?: ButtonProps["sx"];
  children: ReactNode;
}

interface IconProps extends CommonProps {
  onClick?: IconButtonProps["onClick"];
  size?: IconButtonProps["size"];
  color?: IconButtonProps["color"];
  "aria-label"?: string;
  sx?: IconButtonProps["sx"];
  children: ReactNode;
}

/** Default MUI palette for each tier when the button is enabled. The tier
 *  controls the colour: read = primary (the project's green), write =
 *  warning (orange), dangerous = error (red). Callers may still override
 *  with an explicit `color` prop for the rare case where the tier-default
 *  doesn't fit (e.g. color="inherit" on a button inside an Alert). */
function defaultColor(safety: SafetyMode): "primary" | "warning" | "error" {
  switch (safety) {
    case "read":
      return "primary";
    case "write":
      return "warning";
    case "dangerous":
      return "error";
  }
}

/** Wrapping-span styling when the button is forbidden by the current safety
 *  mode: not-allowed cursor on hover, plus a tier-coloured diagonal-stripe
 *  background painted onto the inner disabled button. The stripes are
 *  greyscaled at rest and fade to colour on hover so the resting button
 *  reads as a neutral disabled affordance and the tier reveals itself when
 *  the operator's pointer lands on it. Suppressed when the button is
 *  disabled for non-safety reasons (loading, invalid form, etc) so the
 *  tier styling doesn't lie about why a transient block is in effect. */
function forbiddenSpanSx(
  safety: SafetyMode,
  allowed: boolean,
  externallyDisabled: boolean,
) {
  if (allowed || safety === "read" || externallyDisabled) return null;
  const palette: "warning" | "error" = safety === "write" ? "warning" : "error";
  const angle = safety === "write" ? "135deg" : "45deg";
  return (theme: Theme) => {
    const stripe = alpha(theme.palette[palette].light, 0.24);
    const gap = alpha(theme.palette[palette].light, 0.07);
    return {
      cursor: "not-allowed",
      // MUI's default disabled colour is too pale to read against the
      // striped background, so override it to the normal text colour.
      // Doubled `&&` for specificity over MUI's own .Mui-disabled rule.
      "&& .Mui-disabled": {
        color: "text.primary",
        background: `repeating-linear-gradient(${angle}, ${stripe}, ${stripe} 6px, ${gap} 6px, ${gap} 12px)`,
        filter: "grayscale(0.8)",
        transition: theme.transitions.create("filter", {
          duration: theme.transitions.duration.shortest,
        }),
      },
      "&&:hover .Mui-disabled": {
        filter: "grayscale(0)",
      },
    };
  };
}

function TextActionButton({
  variant,
  safety,
  tooltip,
  startIcon,
  color,
  size,
  type,
  fullWidth,
  sx,
  onClick,
  disabled,
  children,
}: TextProps & { variant: "contained" | "outlined" }) {
  const guard = useGuard(safety);
  const forbidden = !guard.allowed;
  const spanSx = forbiddenSpanSx(safety, guard.allowed, disabled === true);
  return (
    <Tooltip title={tooltip ?? ""}>
      <Box component="span" sx={spanSx ?? undefined}>
        <Button
          variant={variant}
          startIcon={startIcon}
          color={color ?? defaultColor(safety)}
          size={size}
          type={type}
          fullWidth={fullWidth}
          sx={sx}
          onClick={onClick}
          disabled={disabled || forbidden}
        >
          {children}
        </Button>
      </Box>
    </Tooltip>
  );
}

/** Solid contained button. Use for primary page-level actions and dialog
 *  confirms (the destructive variant takes color="error"). */
export function SolidActionButton(props: TextProps) {
  return <TextActionButton variant="contained" {...props} />;
}

/** Outlined button. Use for secondary actions in toolbars and tables. */
export function OutlinedActionButton(props: TextProps) {
  return <TextActionButton variant="outlined" {...props} />;
}

/** Borderless icon button with circular hover. Use for compact row actions. */
export function IconActionButton({
  safety,
  tooltip,
  size = "small",
  color,
  sx,
  onClick,
  disabled,
  "aria-label": ariaLabel,
  children,
}: IconProps) {
  const guard = useGuard(safety);
  const forbidden = !guard.allowed;
  const spanSx = forbiddenSpanSx(safety, guard.allowed, disabled === true);
  return (
    <Tooltip title={tooltip ?? ""}>
      <Box component="span" sx={spanSx ?? undefined}>
        <IconButton
          size={size}
          color={color ?? defaultColor(safety)}
          sx={sx}
          onClick={onClick}
          disabled={disabled || forbidden}
          aria-label={ariaLabel}
        >
          {children}
        </IconButton>
      </Box>
    </Tooltip>
  );
}
