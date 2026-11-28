---
title: Page layout
---
# Page layout

**Source:** `components/ProtectedRoute.tsx`, `components/Navbar.tsx`, `components/EventsSidebar.tsx`, `components/ShellsSidebar.tsx`, `routes/AppDetail.tsx` (`Section`)
· **Preview:** [`./page-layout.html`](./page-layout.html)

```
AppBar position="fixed", Toolbar variant="dense"           48px; primary green (light), #121212 (dark)
 ├ 🌱 + "Seedling" (h6, 700, -0.5px)  → /
 ├ icon links, IconButton small, rgba(255,255,255,0.6), mr 0.5, tooltip only
 ├ flex spacer
 └ fault chip · reconnecting chip · safety chip · host name (body2, monospace, 0.85) · events toggle
Body: flex row, height calc(100vh - 48px)
 ├ ShellsSidebar   outlined square Paper, #1e1e1e, default 600px, drag 300–1400px   (while shells are open)
 ├ main            flex 1, overflow auto
 │   └ Box sx={{ p: 3, maxWidth: 900 | 960 | 1100 | 1400, mx: "auto" }}
 └ EventsSidebar   outlined square Paper, default 340px, drag 220–700px          (while open)
```

## Page header

```
Box display flex, alignItems center, gap 1, mb 2
  Typography h5, flexGrow 1        page title
  IconButton Refresh (tooltip)     optional
  SolidActionButton size small     primary action, last
Typography body2 text.secondary mb 2    optional intro
```

Detail and editor pages put a breadcrumb in place of the title: parent links in `body2` `text.secondary`, `/` separators in `text.disabled`, the current item in `body2`, then the actions.
The app detail page puts the app name in `h5` on the row below, with its status chip, priority control and `gen N` caption.

## Sections

- **List pages** with several tables head each with `Typography subtitle1 sx={{ fontWeight: 600, mb: 1 }}`, an optional `caption` explanation, then the table; sections stack with `Stack spacing={4}`.
- **The app detail page** stacks `Section`s, each an `h6` title row with an optional action at the right, separated by `Divider`s, in `Stack spacing={3}`.
- **Settings** that belong together sit in `Paper variant="outlined" sx={{ p: 2 }}`.

## Do / don't

- ✅ Choose the container width from the four in use.
- ✅ Put the primary action last in the header row.
- ❌ Don't add a page-level banner for state the app bar already shows (safety mode, faults, reconnecting).
- ❌ Don't use `Container`; the authenticated pages centre a `Box` with a `maxWidth`.
