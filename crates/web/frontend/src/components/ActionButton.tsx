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

/** Wrapping-span styling when the button is forbidden by the current safety
 *  mode: not-allowed cursor on hover, plus a tier-coloured diagonal-stripe
 *  background painted onto the inner disabled button. The pattern only
 *  appears on hover so the resting state matches the normal MUI disabled
 *  look. */
function forbiddenSpanSx(safety: SafetyMode, allowed: boolean) {
  if (allowed || safety === "read") return null;
  const palette: "warning" | "error" = safety === "write" ? "warning" : "error";
  return (theme: Theme) => {
    const stripe = alpha(theme.palette[palette].light, 0.32);
    const gap = alpha(theme.palette[palette].light, 0.1);
    return {
      cursor: "not-allowed",
      "&:hover .Mui-disabled": {
        background: `repeating-linear-gradient(45deg, ${stripe}, ${stripe} 6px, ${gap} 6px, ${gap} 12px)`,
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
  const spanSx = forbiddenSpanSx(safety, guard.allowed);
  return (
    <Tooltip title={tooltip ?? ""}>
      <Box component="span" sx={spanSx ?? undefined}>
        <Button
          variant={variant}
          startIcon={startIcon}
          color={color}
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
  const spanSx = forbiddenSpanSx(safety, guard.allowed);
  return (
    <Tooltip title={tooltip ?? ""}>
      <Box component="span" sx={spanSx ?? undefined}>
        <IconButton
          size={size}
          color={color}
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
