---
title: Table
---
# Table

**Source:** MUI `Table`; see `routes/Apps.tsx`, `routes/Keys.tsx`
· **Preview:** [`./table.html`](./table.html)

Lists of things are tables: apps, keys, registries, images, restarts, volumes.
Every table is `size="small"` inside `TableContainer component={Paper} variant="outlined"`.

```
container     outlined Paper; 1px divider border; radius 4px
cell          padding 6px 16px; 14px; line height 24px; bottom rule #e0e0e0 (light), #515151 (dark)
head cell     weight 500; text.primary
action cell   width 40 (one icon) or 80, align right; body cell sx={{ px: 0.5 }}
```

- **Names** in the first column, in monospace when they are identifiers. A name with a description shows the description's first paragraph under it in `caption` `text.secondary`.
- **Secondary cells** (dates, actors, descriptions) are `text.secondary`.
- **States** are [chips](./chip.md) in their own column or beside the name.
- **Clickable rows** navigate to the item's page: `hover`, `onClick`, `cursor: pointer`. Row actions stop propagation.
- **Key/value tables** have no header row; the key cell is 140px wide in `text.secondary`.
- **Empty** tables keep their header and show one row spanning all columns, centred, `text.secondary`, `py: 4`: "No apps registered."

## Do / don't

- ✅ Keep a table's header when it is empty, so the shape of what will appear is visible.
- ✅ Put row actions in the last column, right-aligned, as [icon action buttons](./action-button.md) with tooltips.
- ❌ Don't render a failed read as an empty table; show the error above it.
- ❌ Don't use `size="medium"` or a bare table without the outlined container.
