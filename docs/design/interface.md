# Zephium interface design

Architecture and implemented hosting follow [the frontend contract](../frontend.md).
This document retains earlier visual proposals; use the current contract where they differ.

The design record for the browser's chrome. `frontend.md` is the engineering
contract, `architecture.md` the system. This file is why the interface is
shaped the way it is; read it before proposing a layout change.

## 1. The constraint that shapes everything

Arc and Zen are Firefox and Chromium forks. Their chrome and their page content
live in one compositor tree, so a popover floating over the page costs nothing.
Zephium composes independent native WebViews. Our chrome WebView physically
ends at the sidebar rectangle and cannot paint a single pixel over page content.

This is a design asset, not a handicap. Arc's real failure is not visual: it is
that floating was free, so it floated everything, and now floating panels,
sidebar panels, command bars and mini windows all do overlapping jobs. We do not
get that option, and the discipline it forces is the product.

## 2. Surfaces

Four surfaces. Every element of the interface belongs to exactly one.

| Surface | What it is | Cost | Holds |
| --- | --- | --- | --- |
| Sidebar | The chrome WebView | free | Navigation, identity, tab tree, quick triage of any tool |
| Stage | Content WebViews, including our internal pages | one WebView per page | Web pages, Settings, Notes, History, Task manager, Downloads, Easel |
| Utility | The single panel WebView | ~15 MB, already paid | Launcher, find in page, compact-mode flyout |
| Native menus | muda popup menus | free | Every context menu, every chooser, the app menu |

**Allocation rule.** If it must appear over page pixels it is either a native
menu or the utility surface, and the bar for the utility surface is that nothing
else could work. Everything else is sidebar or stage.

**The utility surface has exactly three admitted jobs**: launcher, find in page,
compact-mode tab flyout. This list is closed. Adding a fourth is a design
decision requiring the same scrutiny as adding a dependency, because the
alternative is always "put it in the sidebar or make it a native menu", and that
alternative is almost always better.

Two properties worth exploiting:

- Internal pages are ordinary stage WebViews, so **notes, history or the task
  manager can split against a live page for free**. Arc needs a bespoke panel
  mode for this. We need the split code we already ship.
- The new tab page renders in the **chrome** WebView, so it costs no WebView
  spawn and shows no paint flash. This makes it the fastest new tab of any
  browser. Protect that; do not move the NTP into the stage.

Native-first applies at the chrome boundary, not inside stage pages. A DOM
`<select>` inside an internal page already renders as a real system menu in
WKWebView, so settings controls need no special machinery.

## 3. Sidebar

```
+------------------------------+
| o o o     <  >  R       [|]  |  header 36px, drag region
| (.) zephium.app         (S)  |  address: identity left, shield right
| [__] [__] [__] [__] [__]     |  essentials, favicon tiles
+------------------------------+
| # Personal                   |  space header, pinned
|                              |
|   pinned tabs                |
|   ------------               |  the only scroll region
|   + New tab                  |
|   open tabs                  |
|   > Archive (12)             |
+------------------------------+
| (@) Crynta               +   |  footer
+------------------------------+
```

**Only the tab lists scroll.** Header, address, essentials and the space header
are pinned. Scrolling the whole sidebar is why the scrollbar reads as belonging
to the nav area; the fix is structural, and hiding the scrollbar comes after.

**Header.** macOS insets the traffic lights; Windows and Linux put min, max and
close at the top right of the sidebar column. There is no other option: on those
platforms the content WebView owns every pixel inward of the 8px window padding,
so a window-level title strip would have to be reserved (costing vertical space
and the silhouette) or drawn over content (impossible). Hover-reveal is rejected
because our top edge is the nav row itself.

**No split button.** Split is entered by dragging a tab onto another tab or a
content edge, from the tab context menu, or from the launcher. Arming a picker
and then clicking a target is a mode, and modes are the enemy.

**Address field.** Identity and permissions dot on the left, blocker shield on
the right. Both are silent when they have nothing to say; the shield gains color
only when protection is off for this site or the policy is degraded. Each opens
a native menu. Blocker diagnostics belong in Settings, Privacy, never in chrome.

**Essentials sit directly under the address field** as favicon tiles with no
labels. The tile shape, not a color, is what separates them from tabs.

**The space header sits above the tabs it governs.** Arc puts its space strip at
the very bottom, hundreds of pixels from the list it labels, which is precisely
why Arc's spaces are hard to reason about. A label goes above its content.
Moving between spaces slides the tab list horizontally, transform only, ~200ms.

**Archive** is collapsed at the foot of the tab list. The three-tier hibernation
already in the backend needs a visible home, and this is the honest answer to
"where did my tab go".

## 4. Compact mode

Two modes only: Default (240px initially, resizable from 180-420px) and Compact
(56px, icons). No hidden mode.

Dragging the Browse sidebar edge moves the column and native page boundary
continuously, including when dragging out of Compact. Intermediate widths are
temporary: release below 140px settles at the compact rail; release at or above
140px settles at an expanded width of 180-420px. The body switches shapes during
the drag without a toggle animation. Cancellation restores the starting width
and shape, and only a completed change saves the mode preference.

**Compact never hover-expands.** An expanding overlay would have to cover page
pixels, which we cannot do, and hover-expand is twitchy regardless.

- Essentials become a vertical icon rail; tabs become favicons with an active bar.
- The address field collapses to a search icon that **opens the launcher**. This
  is what makes the launcher load-bearing rather than decorative, and it is free
  because the panel is already warm.
- The tabs affordance opens the tab list as a flyout on the utility surface.
- Window controls fold into the app menu; a 56px rail has no room.

## 5. Tools

**There is no tools tray.** A row of tool icons in the footer is the Vivaldi and
Arc clutter we exist to beat, and every candidate (notes, tasks, downloads,
media, history, task manager, easel, rules) is a destination rather than a
control. Destinations get two homes and only two.

**The launcher**, for anything nameable. Type "notes", "downloads", "cpu",
"history" and go. This is what earns the panel WebView its memory.

**The sidebar panel slot**, for anything you want visible while browsing. The
sidebar body is a slot that swaps: the default body is tabs, alternates are
Notes, Tasks, Downloads, History and the Settings nav. It replaces; it never
expands and never adds a second column. Escape returns to tabs.

**Any panel promotes to the stage** as a full internal page via its header
button or `Cmd+Enter`. Sidebar is triage, stage is work. This is the answer to
"panel or tab": both, with one explicit gesture between them.

Two things are transient and must never become permanent chrome:

- A starting download slides a progress row above the footer and self-dismisses.
- Playing media shows a compact now-playing row in that same slot.

The full lists remain destinations.

## 6. Profiles and spaces

A space is organizational. A profile is a security boundary with its own SQLite
database and its own WebView data partition. Arc makes them feel like one kind
of object, which is the root of its confusion. **They must look different.**

- **Profile is a footer chip**: avatar plus name, menu opening upward. Profile
  list with a check, New profile, Open incognito window, separator, Settings,
  History, Downloads, Passwords, Quit. On Windows and Linux this chip is also
  the app menu, which removes the need for a separate overflow button.
- Switching crossfades the sidebar with a slight scale, and stays honest: it is
  a real partition swap and the animation must not complete before the backend
  settles.
- **Incognito retints the sidebar** through a per-profile accent token. Felt,
  not read.
- **Space is a header**, never a chip, and lives above its tab list.

## 7. New tab

Renders in chrome. Anchored on **time**, not on the wordmark: a wordmark is the
app talking about itself, the time is useful.

- Large light time, muted date beneath.
- One greeting line from a small curated, translatable set, rotating per new tab
  and never per render.
- Search field, 48px, max-width 560, submitting into the current tab.
- **No essentials grid.** They are already in the sidebar.
- Optional weather top left, off by default, icon and temperature only.
- Tasks count bottom left, expanding in place. Because the NTP is chrome, this
  is a plain DOM popover with no native machinery.
- Background is a per-space token, solid by default. This is also how a space
  earns its identity. No stock imagery; that is Opera's brand, not ours.

Every element is toggleable in Settings, New tab.

## 8. Settings

The sidebar body swaps to `Back to app`, a settings search and grouped
navigation; the content is an internal page in the stage. Groups: General,
Appearance, New tab, Search, Profiles, Spaces, Privacy and security, Passwords,
Downloads, Shortcuts, Boosts, Rules and focus, Performance, Language, Advanced,
About.

## 9. Standing requirements

- **Find in page** and **permission prompts** are the two cases that genuinely
  need to reach past the sidebar. Find uses the utility surface. Permissions
  anchor in the sidebar under the identity dot, which is free for us because our
  omnibox equivalent lives there.
- **Audio state** shows on the tab row; the favicon becomes a speaker on hover.
- **Sleeping tabs must be visible** (reduced favicon opacity). The three-tier
  hibernation is currently silent, which wastes our strongest claim.
- **i18n from the first component.** All strings through a catalog, `Intl` for
  dates and relative time, and CSS logical properties (`ps`/`pe`/`ms`/`me`)
  rather than left and right. Free now, brutal to retrofit.

## 10. Deferred, deliberately

- **Easel.** Highest effort, lowest daily use, and Arc's is dead. If built, it
  is a document type in the notes store, not a subsystem.
- **History graphs and relationship views.** Thin daily utility. Defer the
  feature but **record time-on-site per visit now** so no migration is needed.
- **Autofill.** Large, security-critical, and both engines ship their own.
  Passwords first.
- **Link peek.** One WebView per preview. Not v1.
- **Media as a destination.** Folds into the transient now-playing row.

## 11. Visual system

Hierarchy comes from material, shape, density and typography. Never from
decorative color.

- **Elevation ladder**: canvas, chrome, surface, raised. Each step is a real,
  perceptible change, not three hex points. The active tab row must be
  unmistakable against hover: raised surface, hairline inner border, full
  opacity text, while inactive text drops to muted.
- **Favicons sit on a plate**, desaturated and dimmed when inactive, so mixed
  brightness site icons stop fighting the surface.
- **One optical icon size per row.** Mixed 15/16px in a single control cluster
  reads as an accident.
- **Motion** is transform and opacity only, 120-220ms, and never animates
  browser geometry, startup, or anything the OS is also animating.
- Every surface must hold up in dark, light, reduced motion and forced colors.

## 12. Implemented component system

The [frontend contract](../frontend.md) governs ownership, native boundaries,
testing and migration status. [The interface system](system.md) records the shared
visual language. Interface Studio and the unused Work fixture kit are removed;
Settings is the current surface for inspecting controls. Liquid Glass is native
window material, not an additional overlay surface.
