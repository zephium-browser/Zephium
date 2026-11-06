# Zephium interface system

The implementation follows `product-system.md`: Browse stands alone; Work
makes useful state visible. Geist, neutral surfaces, optical alignment, and
restrained motion give both environments one identity. The system lives in
`frame/src/shared/ui`, with semantic tokens and shared styles in `frame/src/styles`.

## Inspect the interface

Run `pnpm dev` and open Settings from the browser's profile menu. Settings is
part of the actual browser window, with navigation in the sidebar and its
content in the main area. Interface Studio and its separate entry points have
been removed. See [Settings interface](settings.md) for the current section
map, preview-state contract, and language-selection behavior.

Appearance saves Sidebar position as `sidebar.position`, defaulting to Left.
Right mirrors the sidebar/content arrangement while tools and Settings stay
inward. Hidden reveal follows the selected edge. Window controls remain on
their native OS sides.

## Tokens and composition

Use semantic colors, never raw palette utilities in product markup. The
surface ladder is canvas → surface → raised; transient chrome fills remain
transparent overlays on a real native material. Material chrome uses stronger
foreground tokens to remain legible over varying backgrounds. The original browser controls are the styling reference. Do not add decorative color gradients or glow.
CSS blur and backdrop filters follow `frontend.md`: allowed where they improve the
interface within measured cost, never gratuitous or animated.

The rhythm is 8 px with 4 px subdivisions. Chrome controls are 28–36 px; general
buttons also have a 40 px large size. Icons are optically consistent at 16 px in
control clusters. Radius is named by role, never by size: `--radius-inset` (8),
`--radius-row` (12, a row in a column and any field sharing that column),
`--radius-control-compact` (10), `--radius-control` (14), `--radius-control-large`
and `--radius-card` (18), `--radius-panel` (22) and `--radius-capsule`. Tailwind's
size-named radius scale is removed from the theme, and `check-styles` fails on a
`rounded-md`-style class, which would otherwise compile to a square corner. The
native page view uses the continuous (squircle) corner curve. The native content
radius and startup canvas colors remain shared contracts, not freely adjustable
theme knobs. Body type is 13 px, labels 12 px, captions 11 px; page titles and
specimen display sizes are intentionally larger.

Buttons and inputs use native HTML semantics. Form submit buttons opt into
`type="submit"`. Inputs have generated stable IDs and associated help/error
text. Native selects and radio groups retain platform keyboard behavior. No
new overlay behavior or cross-WebView portals are introduced.

| Component | Contract |
| --- | --- |
| Button | `variant`: primary/secondary/ghost/danger; `size`: compact/regular/large; `pending` disables activation and exposes busy state; native button attributes pass through |
| IconButton | Existing icon/label/menu contract plus optional pending state; keep labels descriptive |
| Field / TextArea | Required label, bindable string value, optional hint/error; native input attributes pass through |
| SearchField | Required accessible label, bindable query, trimmed `onsubmit(value)`; does not navigate by itself |
| Toggle | Checkbox or switch, label/description, bindable checked value, disabled, `onchange(checked)` |
| Select / SegmentedControl | Typed value/label options, bindable value; SegmentedControl is the single presentation for a mutually exclusive choice |
| Badge | Neutral/accent/success/warning/danger with an optional nonsemantic status dot; always include status text |
| Surface | Canvas/surface/raised; layout belongs to the caller |
| ListRow | Selection button with label, description, leading/trailing snippets and callback; do not nest interactive controls |
| Progress | Label, optional value, max; native progress semantics |
| EmptyState | Title, description, optional icon/action snippets |
| Separator / KeyHint | Semantic divider and platform-specific shortcut presentation |

All these components are presentation-only. They do not import domain stores,
call IPC, persist settings, fetch resources, or claim native success. For live
operations, the parent holds authoritative state and waits for settlement. A
checkbox changing in a development fixture is not a durable domain commit.

## Browse and Work

Live Browse uses the shared icon control, common action styles in consent
surfaces, clarified active-tab treatment, stronger address/search focus,
consistent navigation icon sizing, and the quieter new-tab typography. Existing
native menus, extension behavior, permission focus traps, sidebar structure,
compact-mode address sentinel, and tab presentation barriers remain intact.
The authoritative title sentinel still contains only the exact tab title.

The unused Work fixture kit has been removed during the frontend foundation
migration. Future Work and entity presentations will use the same shared primitives,
with state supplied through the real Rust contracts. No production Work workflow
is implemented by this UI foundation.

## Tasks

Tasks are a product surface rather than a resource browser. The row is the whole
interaction: capture happens in a field shaped like a row at the top of the list,
completion is the kit's checkbox recipe, and scheduling opens a contained popover
on the row itself. Nothing navigates away from the list, and no editor route
replaces it.

The list is grouped by when work is due — overdue, today, tomorrow, upcoming,
anytime, completed — and the segmented control selects a scope rather than a
filter, opening on Today. A task captured while looking at a scope lands in it.
Rows carry the sidebar row vocabulary (`--row-hover`, `--row-active`,
`--shadow-raised`, the mask-faded label), so a task row and a tab row read as one
product. Colour is spent only on time that has run out and on a delegated task
that stopped and needs a person; everything else stays quiet.

A completed row holds its place for a moment before leaving, animating opacity
and transform only. Status indicators are static: a list that animates
continuously is a list nobody can read. Arrow keys move the selection, Space
completes, Enter opens the row's detail in place, Cmd-click and Shift-click
build a selection that Space and Delete then act on together, and the panel owns
undo so it survives the deletion that emptied the list. Every one of those
changes is announced in a polite live region, because the row moving is only
feedback for a reader who can see it.

The capture field reads what was typed before committing it. A date or a time at
the end of the line — "Call Anna tomorrow at 3pm" — is understood, shown back as
a chip, and removed from the title; pressing the chip or Escape keeps the words
instead. Only a phrase at the very end is ever consumed, so "Meet the Friday
team" keeps its Friday. The vocabulary lives in `features/tasks/lib/task-language.ts`
and is tested against the false positives that make such a parser hated.

The panel spends one band on chrome. The tool's title is its scope — "Today" —
and opens a menu holding the others; search is summoned from the overflow rather
than resident, because in a 336px column a permanent field costs more than it
returns. Three controls sit beside it: capture, overflow, close.

A row's date appears only when it says something its section has not. Under a
heading that reads TODAY, every row repeating "Today" is noise wearing the
costume of information. The chip stays reachable either way — it is how a date
is changed — but it speaks only for an hour, for being late, or for a day the
section does not already name.

The list is windowed. Rows declare their height rather than being measured,
which keeps the window a prefix sum, and the detail's box is fixed for the same
reason. The finished pile is clipped to the most recent and says how much it is
hiding. A search widens past the current scope, because a scope must never veto
the answer to "where is it".

## The tasks destination

`browser.tasks` opens the full page, reached from the tools menu (Show All
Tasks) and from the panel's own overflow. It follows Settings: the browser
sidebar becomes its navigation and the main area holds the content. The same
rows draw there at `page` density, so the panel and the destination cannot drift.

The page states what it is: the scope as a heading, and one line saying what the
scope amounts to. Given room it is two panes — the list, and the whole task
beside it with space for its notes. That composition is the reason a full mode
exists, since a 336px column cannot have it; below 1140px the pane gives way and
rows open in place again. A listing carries no body, so opening a task reads its
own — one record, for the one row being looked at.

The board groups by state — To do, In progress, Needs you, Done — rather than by
day, because that is the axis where dropping a card performs a mutation the model
already has. Dragging between columns sets the status; dragging within one writes
the manual position. The gesture is the sidebar's own: a movement threshold so a
click stays a click, pointer capture, a frame-throttled ghost, and a drop target
read from the element under the pointer. It lives in `shared/lib/pointer-drag.svelte.ts`
rather than in a dependency.

Positions are fixed-width decimal keys, so comparing them as text orders them as
numbers. An insert takes the midpoint of its neighbours — one write, not a
renumbered column — and when that gap is finally spent `orderBetween` says so
instead of colliding, and the column is resequenced.

## Localization and motion

New-tab, navigation, address and tab-action strings use compiled Paraglide
messages from `frame/messages/en.json`. The pinned message-format plugin loads
from local dependencies; compilation needs no remote plugin download. `pnpm -C
frame i18n` regenerates ignored output, and typecheck/build run it automatically.
Components receive localized strings from their owners. Existing untouched
feature copy remains an incremental localization migration, not a claim of a
fully translated product. Text expansion and locale stress belong in development tests, never in persisted fake locale settings.

### Motion

Motion is named by what it is for. `--motion-instant` (70 ms) answers the
pointer; `--motion-fast` (120), `--motion-base` (200) and `--motion-slow` (300)
are for things that change in place or travel; `--motion-page` (400) is for a
surface arriving. Curves: `--ease-out` and `--ease-smooth` for feedback,
`--ease-emphasized` for arrivals, `--ease-exit` for leaving (always quicker than
arriving), `--ease-spring` (a damped spring sampled with `linear()`, 2%
overshoot) for small things that move, and `--ease-snap` (a launched spring,
1.4%) for what answers a click. Scripted motion reads the same tokens through
`shared/lib/motion.ts`, so declared and scripted motion cannot drift.

Everything that moves animates `transform` or `opacity`, measured once before a
change and once after it:

- `shared/lib/list-motion.ts` moves a keyed list: moved items slide from where
  they were, new ones rise, departed ones fade as inert ghosts stripped of every
  `data-zephium-*` sentinel (native must never find a closed tab), and an item
  leaving one list for another carries its mark across. Only a change of order
  or membership is measured; title and loading changes cost nothing.
- `shared/lib/plate-glide.ts` carries a selection's plate to the next selection
  (tabs, settings pages) inside a `data-glide-host`.
- `features/sidebar/lib/shape-morph.ts` turns the list into the rail and back:
  every mark travels, the old column fades as a ghost, and the column's width
  travels on the page curve while its contents hold their final width.
- `session/row-drag.svelte.ts` is the one drag gesture for rows, rails and kept
  sites; rows reorder in place by transform and land through the list's motion.

Native geometry moves too, and never lays a page out per frame. A deliberate
change of shape (a toggle, a snap, a tool opening) is sent with a travel flag;
the macOS stage keeps whichever of its two frames is wider for the journey and
animates only its layer's translation, so a page is laid out once. A chrome that
would narrow keeps its width until the page has arrived. Returning from a browser
page fades and settles the page back in. Windows moves its page windows on the
same curve and the same wider-frame rule. A drag, a restored preference and a
window resize never travel.

The launch cascade is armed before the first frame and runs when native reveals
the window, so a row is never seen before it arrives. Reduced motion (system or
in-app) collapses all of it. Nothing polls; the only continuous animation is a
page's loading arc, which turns by transform alone.

## Native material

`UiInfo.material` is a generated enum: none, vibrancy, liquid_glass, acrylic, mica.
It represents the effect installed on the requesting window. `material.*`
messages on the existing scoped UI-command transport update it at runtime;
a later startup query cannot overwrite an already received update.

macOS uses window-vibrancy 0.8.0: public Regular Liquid Glass on supported
macOS 26+, Sidebar/HudWindow vibrancy as fallback, and opaque CSS surfaces if
neither installation succeeds. Reduce Transparency skips effects entirely.
An AppKit accessibility observer reapplies the material on settings changes;
observers and per-window records are removed on destruction. All NSView work
stays on the main thread.

The material is installed below the existing Tao content-view children, with
no `content_view()` reparenting. WKWebView parent bounds, coordinate translation,
page sibling order and the chrome rectangle remain unchanged. Native theme
changes update AppKit, rather than merely retinting HTML. The launcher retains
its existing 20 px radius and the main window its existing 12 px radius.
Windows keeps its Acrylic policy with per-window reporting; Linux stays solid.
The enum includes Mica for truthful platform reporting, not a new Windows policy.

## Original browser controls are the visual baseline

The rejected material-study stylesheet has been removed. Live
Browse uses the original semantic palette; there is no independent gallery
palette or control skin.

The original `IconButton` styling and address/new-tab input wrappers are the
reference. Buttons have quiet resting states, existing rounded-rectangle
radii, and translucent hover/press feedback. Text Button extends this language
with padding and semantic primary/secondary variants. Inputs use the original
34 px address-field geometry, fill, and faint inset highlight. SearchField also
provides a `page` size matching the original 48 px new-tab search. Keyboard
focus, labels, validation, and disabled behavior remain explicit.

SettingsGroup/SettingsRow supply grouped composition. Switches keep their bright
thumb and native-like proportions. Segmented selection uses the original
selected-surface fill and 240 ms transform travel; no palette override or
measurement loop is needed. Reduced motion and RTL remain supported.
