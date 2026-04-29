import {
  Button,
  IconButton,
  Tooltip,
  type ButtonProps,
  type IconButtonProps,
} from "@mui/material";
import { type ReactNode } from "react";
import { type SafetyMode, useGuard } from "./SafetyModeProvider";

interface CommonProps {
  /** Tier required to invoke. "read" buttons are always allowed; "write" and
   *  "dangerous" disable themselves outside the matching safety mode and
   *  show a coloured dashed/dotted outline so the requirement is visible at a
   *  glance, with a not-allowed cursor on hover. */
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

/** Forbidden styling: tier-specific dashed (write) or dotted (dangerous)
 *  outline in the matching faded palette colour. Outline doesn't affect
 *  layout, so allowed/forbidden states have identical metrics. */
function forbiddenSx(safety: SafetyMode, allowed: boolean, useBorder: boolean) {
  if (allowed || safety === "read") return null;
  const borderStyle = safety === "write" ? "dashed" : "dotted";
  const borderColor = safety === "write" ? "warning.light" : "error.light";
  if (useBorder) {
    return {
      "&.Mui-disabled": { borderStyle, borderColor },
    };
  }
  return {
    "&.Mui-disabled": {
      outlineStyle: borderStyle,
      outlineColor: borderColor,
      outlineWidth: "1px",
      outlineOffset: "-3px",
    },
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
  // Outlined buttons already have a solid border, so swap its style/colour
  // rather than stacking an outline on top.
  const safetySx = forbiddenSx(safety, guard.allowed, variant === "outlined");
  return (
    <Tooltip title={tooltip ?? ""}>
      <span style={forbidden ? { cursor: "not-allowed" } : undefined}>
        <Button
          variant={variant}
          startIcon={startIcon}
          color={color}
          size={size}
          type={type}
          fullWidth={fullWidth}
          sx={[safetySx, sx].filter(Boolean) as ButtonProps["sx"]}
          onClick={onClick}
          disabled={disabled || forbidden}
        >
          {children}
        </Button>
      </span>
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
  const safetySx = forbiddenSx(safety, guard.allowed, false);
  return (
    <Tooltip title={tooltip ?? ""}>
      <span style={forbidden ? { cursor: "not-allowed" } : undefined}>
        <IconButton
          size={size}
          color={color}
          sx={[safetySx, sx].filter(Boolean) as IconButtonProps["sx"]}
          onClick={onClick}
          disabled={disabled || forbidden}
          aria-label={ariaLabel}
        >
          {children}
        </IconButton>
      </span>
    </Tooltip>
  );
}
