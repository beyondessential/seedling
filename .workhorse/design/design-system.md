---
title: Seedling Design System
---
# Seedling Design System

The [operator console](../specs/web/console.md) is a single-page app for running a host: listing apps, reading their faults and logs, changing parameters, opening shells.
Its users are operators who know the system and are often in the middle of fixing something.
The interface is a dense, plain tool: tables of names and states, small chips, monospace identifiers, and a safety mode that keeps destructive controls out of reach until asked for.
Nothing is decorative.
Colour is spent on state, so a screen that is mostly grey and green with one red chip says where to look.

The console is built on MUI with almost no theming: a green primary, a brown secondary, and MUI's defaults for everything else.
The values below are what MUI resolves to and what the screens render; read them from the theme in code (`primary.main`, `text.secondary`, `theme.spacing`), never as literal hexes.

The [snapshots](snapshots/) are faithful captures of the shipped screens, and the [component docs](components/) cover the recurring parts.

## Colour

The theme lives in `crates/web/frontend/src/theme.ts` and sets only the palette mode and the primary and secondary colours.
Light or dark follows the operating system's `prefers-color-scheme`; there is no manual switch.

### Theme colours

| Slot | Light | Dark | Used for |
|---|---|---|---|
| `primary.main` | `#2e7d32` | `#4caf50` | The app bar (light), contained buttons, links in buttons, focus, read-tier actions |
| `secondary.main` | `#6d4c41` | `#a1887f` | Unused. Leave it unused |
| `success.main` | `#2e7d32` | `#66bb6a` | Healthy, running, satisfied, raised priority |
| `info.main` | `#0288d1` | `#29b6f6` | In progress: installing, operating, a running gate, discovered |
| `warning.main` | `#ed6c02` | `#ffa726` | Needs attention: degraded, stopped, lowered priority, write tier |
| `error.main` | `#d32f2f` | `#f44336` | Faulted, failed, expired, fault counts, dangerous tier |

Success and primary are the same green in light mode, so a success chip and a primary button read alike.
Keep primary for actions and success for state, and the context separates them.

### Neutrals

| Token | Light | Dark |
|---|---|---|
| `background.default` / `background.paper` | `#ffffff` | `#121212` |
| `text.primary` | `rgba(0,0,0,0.87)` | `#ffffff` |
| `text.secondary` | `rgba(0,0,0,0.6)` | `rgba(255,255,255,0.7)` |
| `text.disabled` | `rgba(0,0,0,0.38)` | `rgba(255,255,255,0.5)` |
| `divider` (also outlined Paper borders) | `rgba(0,0,0,0.12)` | `rgba(255,255,255,0.12)` |
| Table cell rule | `#e0e0e0` | `#515151` |
| Default chip fill | `rgba(0,0,0,0.08)` | `rgba(255,255,255,0.16)` |

### Fixed colours outside the theme

Two surfaces keep their own colours whatever the mode:

- **The app bar's controls** sit on the green bar in light mode and the dark surface in dark mode, so its icons are `rgba(255,255,255,0.6)` at rest and white when active, and the read-only chip is outlined in `rgba(255,255,255,0.5)`.
- **Terminals** (the shells sidebar and the full-page shell) are a dark, editor-like panel in both modes: background `#1e1e1e`, tab strip `#252526` with `#3c3c3c` rules, tab text `#999` and `#ccc` when selected, indicator `#007acc`, exit overlay `rgba(0,0,0,0.75)` with `#cccccc` text.
- **Log stderr lines** are tinted `rgba(255,100,0,0.06)`, with the stream label in `warning.main`.

### Rules

- **Colour means state.** Every semantic colour carries the meaning in the table above and nothing else; don't use red for emphasis or green as an accent.
- **Read colours from the theme.** Use palette keys (`"warning.main"`, `color="error"`), not hexes, so dark mode follows. The fixed colours above are the only exceptions.
- **Don't tint chrome.** Surfaces are `background.paper` with a `divider` border; group with spacing and headings, not coloured panels.

## Status vocabulary

State is shown as a small chip in the colour of its meaning.
The mappings live in `crates/web/frontend/src/lib/status.ts` and beside the components that use them; new states join the existing scale rather than adding colours.

| Meaning | Colour | Examples |
|---|---|---|
| Healthy, done, in use | success | `running`, gate `satisfied`, healthcheck passing, certificate `active`, image `in use`, raised priority (`high`, `critical`, `elevated`) |
| In progress, informational | info | `installing…`, `operating: gate migrate`, gate `running`, `scheduled ×N`, discovered ingress |
| Needs attention | warning | `degraded`, `stopped`, gate `unsatisfied` or `paused`, lowered priority (`low`), `expires soon`, `partially running`, recent restarts |
| Broken | error | `faulted`, failed, `expired`, healthcheck failing, the fault count |
| Neutral, absent | default | `not installed`, `uninstalling`, `normal` priority (not shown on the apps list), undetermined |

Filled chips carry an entity's primary state (its status, lifecycle, in use).
Outlined chips carry attributes and tags (kind, protocol, `ro`, `lb: round_robin`, the fault count on a row).
Status labels are the state's own name in lower case, with underscores as spaces.

## Typography

The font stack is MUI's default, `Roboto, Helvetica, Arial, sans-serif`.
No font is bundled or loaded, so the console renders in Roboto where the system has it and Helvetica or Arial otherwise.
The base size is 16px on the body; most text on screen is `body2` at 14px.

| Role | Variant | Size / weight / line height |
|---|---|---|
| Page title | `h5` | 24px / 400 / 32px |
| Section heading on the app detail page | `h6` | 20px / 500 / 32px |
| Section heading on list pages | `subtitle1`, `fontWeight: 600` | 16px / 600 / 28px |
| Sidebar heading, resource name | `subtitle2` | 14px / 500 / 22px |
| Body, table cells, intro paragraphs | `body2` | 14px / 400 / 20px |
| Help text, timestamps, metadata | `caption` | 12px / 400 / 20px |
| Chip label | | 13px |
| Mini chip label | | 0.65rem (about 10px), chip height 18px |

The "Seedling" wordmark in the app bar is `h6` at weight 700 with `-0.5px` letter spacing; it is the only bold `h6`.
There are no `h1` to `h4` variants in the app; the sign-in and offline cards use `h5` rendered as an `h1` element.
Button labels are upper case, MUI's default, at 14px weight 500; write them in sentence case in code and let the theme transform them.

### Monospace

Identifiers render in the browser's `monospace` family: app, resource, instance, gate, action and registry names; fingerprints, serials, digests and hashes; cron expressions; the hostname in the app bar; fault messages; log lines; dialog titles that name an entity.
Identifier inputs set the same family on the input element.
Sizes step down for long machine strings: fingerprints and serials at 0.8rem, PEM text and stack traces at 0.75rem, event summaries at 0.72rem, log lines at 0.78rem with line height 1.4.
Truncated IDs show their first 12 characters.

## Spacing

The unit is MUI's 8px, used through `sx` shorthands (`p: 3` is 24px).
The ladder in use:

| Value | Typical use |
|---|---|
| `0.5` (4px) | Gaps inside a chip row, between an icon and its label |
| `1` (8px) | Gaps between controls in a header row, below a section heading |
| `2` (16px) | Below a page header, between fields (`Stack spacing={2}`), Paper card padding |
| `3` (24px) | Page padding, between sections on the app detail page |
| `4` (32px) | Between sections on list pages with several tables |

## Shape and elevation

- Radius is MUI's 4px on buttons, inputs, Paper and alerts; chips are fully round (16px on a 24px chip).
- Surfaces are flat: `Paper variant="outlined"` with a 1px `divider` border.
- The app bar keeps MUI's elevation shadow, and the sign-in and offline cards are `Paper elevation={3}`. Nothing else is raised.

## Layout

The [page layout](components/page-layout.md) doc has the measurements.

- **App bar.** Fixed, dense (48px), primary green in light mode. Left to right: the 🌱 mark and "Seedling" wordmark linking home, then one small icon link per area (sessions, keys, registries, Canopy, images, services, ingresses, certificates, volumes, templates, restarts), each labelled only by its tooltip. On the right: the fault count chip (only when there are faults), the reconnecting chip (only while reconnecting), the safety mode chip, the host name in monospace, and the events sidebar toggle.
- **Body.** A flex row under the bar, filling the viewport: the shells sidebar on the left while shells are open, the page in the middle (scrolling on its own), the events sidebar on the right while open. The sidebars are outlined square Paper panels, resizable by a 4px drag handle on their inner edge, with their open state and width kept in local storage.
- **Page container.** `Box sx={{ p: 3, maxWidth, mx: "auto" }}`, centred. Width by content: 900px for lists and the app detail page, 960px for editors, 1100px for wide tables (keys, restarts, images, certificates), 1400px for ingresses. Log and shell pages are full-bleed.
- **Page header.** A flex row with `mb: 2`: the `h5` title growing to fill, then the page's actions. An intro paragraph in `body2` `text.secondary` may follow.
- **Breadcrumbs** on detail and editor pages: parent links in `body2` `text.secondary`, underlined on hover, separated by `/` in `text.disabled`.
- **Sections.** List pages stack sections with a `subtitle1` heading at weight 600; the app detail page stacks `h6` sections separated by dividers. Grouped settings sit in an outlined Paper card with `p: 2`.

### Rules

- In a page header, the page's primary action comes last, at the right edge, with any refresh icon button before it.
- New pages pick one of the existing container widths.
- A page that lists one kind of thing is a table, not a stack of cards.

## Components

| Part | Doc |
|---|---|
| Action buttons and the safety tiers | [action-button.md](components/action-button.md) |
| Status and attribute chips | [chip.md](components/chip.md) |
| Tables | [table.md](components/table.md) |
| Dialogs and confirmations | [dialog.md](components/dialog.md) |
| Page layout, header and sections | [page-layout.md](components/page-layout.md) |

## Safety mode

The [guarded controls](../specs/web/console.md#guarded-controls) the spec requires have one visual language, built from `safetyStripeBackground` in `SafetyModeProvider.tsx`:

- **Tier colour.** Read is primary, write is warning, dangerous is error. An action button's colour is its tier's.
- **Stripes.** A tier is also a diagonal stripe in its light colour: write leans 135°, dangerous 45°, 6px stripes at 24% alpha over 6px gaps at 7%. The angle tells the two apart without colour.
- **The mode chip** in the app bar is outlined white in read-only, and filled with the tier colour plus stripes when elevated, with the time left in monospace (`Write · 9m`).
- **Forbidden controls** stay visible but disabled, with stripes greyscaled at rest that colour in on hover and a not-allowed cursor. A control disabled for another reason (loading, an invalid form) shows MUI's plain disabled style.
- **Another operator elevated** adds an outlined `Others: Write` chip before the mode chip, and outlines the mode chip in the highest tier's colour.

There is no page banner for the mode.

## Feedback states

- **Loading.** A centred `CircularProgress` with `mt: 4` for a page, an inline one at 20px for a section, 14px or 16px inside a button beside a busy label. No skeletons or progress bars.
- **Busy labels** end in an ellipsis character: `Saving…`, `Planning…`, `Uninstalling…`.
- **Errors.** A failed request shows inline above the content it concerns, through `OiErrorAlert`: an error alert reading `[OI] method: [code] message`, with the stack trace in a collapsed `details`. A failed read is never shown as an empty list.
- **Faults** are error alerts with the whole message in monospace, plus clickable `logs: target` chips.
- **Alerts with actions** carry their buttons in the alert's `action` slot: an operation in progress with Cancel, staged parameters with Plan, Apply and Discard.
- **Empty states** are one line in `text.secondary`, "No apps registered.", inside the table as a centred row where the table has a header, otherwise in place of the table. An empty state that blocks the system (no authorised keys, an empty registry allowlist) is a warning alert saying what cannot happen until it is filled.
- **Missing values** show `—`.

## Iconography

Icons are `@mui/icons-material`, filled style, at `fontSize="small"` in the app bar and tables and 14 to 16px inside dense rows.
An icon button always has a tooltip, which is its only label.
Reuse the existing meaning for an icon before choosing a new one:

| Meaning | Icon |
|---|---|
| Refresh, restart, renew | `Refresh` |
| Add, create, scale up | `Add` |
| Remove, delete, revoke | `DeleteOutlineOutlined` |
| Edit | `Edit` |
| Logs | `Article` |
| Run, unstop, follow | `PlayArrow` |
| Stop a resource, pause following | `Pause` |
| Stop a shell or forward | `Stop` |
| Cancel an operation | `Cancel` |
| Unmap, detach | `LinkOff` |
| Shell | `Terminal` |
| Snapshot | `CameraAlt` |
| Safety modes | `Lock`, `Shield`, `Warning` |

## Copy

- **Spelling** is New Zealand English: "Authorise", "colour", "organisation".
- **Buttons** are a verb or verb phrase in sentence case in code: "New app", "Authorise key", "Review & create". A button that opens a further step ends in `…` ("Open shell…").
- **Headings** are sentence case: "Site ingresses", "Generation history". Several list pages still use Title Case ("Container Images", "Held Volumes"); new headings don't.
- **Empty states** are "No <things>." with a full stop.
- **Confirmations** ask a question naming the thing, then say the consequence: title "Remove template?", body "Remove `name`? Apps already instantiated from it are unaffected." An irreversible action says so: "This cannot be undone." The confirm button repeats the verb ("Remove", "Revoke"), never "OK" or "Yes".
- **Times** are the browser's locale format for absolute times, and "5m ago" or "in 5d" for relative ones with the absolute time on hover.
- **Sizes** are binary units: B, KiB, MiB, GiB.

## Dark mode

Everything above that reads from the theme follows the operating system into dark mode; check a new surface in both.
Code views pass the theme's mode to CodeMirror (`theme={mode}`) so they follow it too.
