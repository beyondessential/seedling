---
title: Chip
---
# Chip

**Source:** MUI `Chip`; colour mappings in `crates/web/frontend/src/lib/status.ts`
· **Preview:** [`./chip.html`](./chip.html)

Chips carry state and attributes beside a name.
Every chip is `size="small"`, and its colour follows the [status vocabulary](../design-system.md#status-vocabulary).

| Kind | Variant | Example |
|---|---|---|
| Primary state of the thing | filled | `running`, `degraded`, `not installed`, `in use`, `active` |
| Attribute or tag | outlined | `http`, `ro`, `lb: round_robin`, `normal priority` |
| Count that needs attention | outlined, error | `1 fault` on an apps row |
| Navigation | filled or outlined, `component={Link} clickable` | `3 faults` in the app bar, `logs: web` on a fault |
| Mini | outlined, 18px high | Event types in the events sidebar, resource badges on the app detail page |

```
small chip   height 24px; label 13px; label padding 0 8px; radius 16px
mini chip    height 18px; label 0.65rem; label padding 0 6px (sx: { fontSize: "0.65rem", height: 18, "& .MuiChip-label": { px: 0.75 } })
outlined     1px border at 70% of the colour; text in the colour
default      filled rgba(0,0,0,0.08) light, rgba(255,255,255,0.16) dark
```

Labels are lower case for states, as the state is named (`operating: gate migrate`), and monospace in the app bar and on fault log links.

## Do / don't

- ✅ Map a new state into the existing colours by meaning, in `lib/status.ts` or beside the component.
- ✅ Leave out a chip whose value is the default (`normal` priority on the apps list, zero faults).
- ❌ Don't use a chip as a button for an action; actions are [action buttons](./action-button.md).
- ❌ Don't add a colour outside success, info, warning, error, default and, for pinned or strategy tags only, primary.
