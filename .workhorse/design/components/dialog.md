---
title: Dialog
---
# Dialog

**Source:** MUI `Dialog`; confirm helper `ConfirmDialog` in `routes/Certificates.tsx`
· **Preview:** [`./dialog.html`](./dialog.html)

Dialogs hold forms that create or change something, reviews of a planned change, and confirmations.

```
<Dialog open onClose={busy ? undefined : close} maxWidth="sm" fullWidth>
  DialogTitle          20px / 500; padding 16px 24px
  DialogContent        padding 0 24px 20px (dividers: 16px 24px with rules)
    Stack spacing={2} sx={{ mt: 1 }}   fields, OiErrorAlert on failure
  DialogActions        padding 8px; 8px gap; right-aligned
    Button (text) Cancel    SolidActionButton confirm
```

| Width | Use |
|---|---|
| `xs` (444px) | A confirmation, or a form of one field |
| `sm` (600px) | Most forms |
| `md` (900px) | Reviewing a plan or a script, with `DialogContent dividers` |
| `lg` (1200px) | Uploading a template with its editor |

- **Title.** A question for a confirmation ("Remove template?"), the action for a form ("Authorise OI key"). An entity's name in the title is monospace: "Plan change · `hello-world`".
- **Body of a confirmation.** "Remove `name`?" then the consequence in one sentence, and "This cannot be undone." when it cannot.
- **Actions.** Cancel as a text button on the left, the confirm as a `SolidActionButton` on the right with the action's verb as its label and the action's tier as its `safety`.
- **While busy** the Cancel button and backdrop close are disabled and the confirm shows a spinner and an ellipsis label.
- **Errors** appear inside the dialog, above the actions, as an `OiErrorAlert`; the dialog stays open.

## Do / don't

- ✅ Pass `fullWidth` with every `maxWidth`.
- ✅ Keep a dangerous confirm's colour coming from `safety="dangerous"`.
- ❌ Don't label the confirm "OK", "Yes" or "Confirm".
- ❌ Don't close a dialog on a failed request.
