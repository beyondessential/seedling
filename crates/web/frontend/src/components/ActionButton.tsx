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
   *  "dangerous" disable themselves outside the matching safety mode and show
   *  the tier as a coloured prefix in the tooltip. */
  safety: SafetyMode;
  /** Action description appended after the tier prefix in the tooltip. When
   *  safety="read" and tooltip is omitted, no tooltip is shown. */
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
  const title = guard.title(tooltip);
  return (
    <Tooltip title={title ?? ""}>
      <span>
        <Button
          variant={variant}
          startIcon={startIcon}
          color={color}
          size={size}
          type={type}
          fullWidth={fullWidth}
          sx={sx}
          onClick={onClick}
          disabled={disabled || !guard.allowed}
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
  const title = guard.title(tooltip);
  return (
    <Tooltip title={title ?? ""}>
      <span>
        <IconButton
          size={size}
          color={color}
          sx={sx}
          onClick={onClick}
          disabled={disabled || !guard.allowed}
          aria-label={ariaLabel}
        >
          {children}
        </IconButton>
      </span>
    </Tooltip>
  );
}
