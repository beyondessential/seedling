---
title: Action button
---
# Action button

**Source:** `crates/web/frontend/src/components/ActionButton.tsx`
· **Preview:** [`./action-button.html`](./action-button.html)

Every control that calls the Operator Interface is an action button: a MUI button that knows its [safety tier](../design-system.md#safety-mode) and disables itself when the console's mode is below it.
Plain MUI `Button` is for controls that change nothing on the host: Cancel, Close, Load file, Show older.

## Variants

| Component | MUI variant | Use |
|---|---|---|
| `SolidActionButton` | contained | The page's primary action, and the confirm button of a dialog |
| `OutlinedActionButton` | outlined | Secondary actions in toolbars, and per-row actions with a label (Run, Map, Renew) |
| `IconActionButton` | icon button, small | Compact row actions; the tooltip is its label |

| Prop | Purpose |
|---|---|
| `safety` | `"read"`, `"write"` or `"dangerous"`: the mode the action needs, and its colour |
| `tooltip` | Shown on hover; required on `IconActionButton` |
| `color` | Overrides the tier colour, for the rare button inside an alert (`"inherit"`) |
| `size` | `"small"` in page headers and rows; `"medium"` in dialogs |

```
tier        colour            enabled when mode ≥ tier
read        primary green     always
write       warning orange    Write or Dangerous
dangerous   error red         Dangerous
```

A button whose tier is above the current mode stays in place, disabled, with the tier's stripes greyscaled and a not-allowed cursor; on hover the stripes colour in.
A button disabled for another reason (busy, invalid form) uses MUI's plain disabled style, so the stripes never claim a mode is the reason when it is not.

```
contained small   13px / 500 / upper case; padding 4px 10px; radius 4px; MUI elevation-2 shadow
contained medium  14px; padding 6px 16px
outlined small    1px border at 50% of the tier colour; padding 3px 9px
icon small        padding 5px; circular hover tint
```

## Do / don't

- ✅ Give every action the lowest tier the [guarded controls](../../specs/web/console.md#guarded-controls) allow: changes are `write`, removals, revocations, stops, cancels and clears are `dangerous`.
- ✅ Put a busy action's spinner and an ellipsis label inside the button: `Saving…`.
- ✅ Use `startIcon={<AddIcon />}` on create buttons in page headers.
- ❌ Don't use `Button variant="contained" color="error"` for a destructive confirm; use `SolidActionButton safety="dangerous"` so the mode guards it.
- ❌ Don't hide a control the mode forbids.
