# Browsing history and site icons

History is one module, `frame/src/features/history`, behind two thin hosts: the
full-window library reached from the History menu, and the sidebar tool panel.
Both read the same session and render the same list.

## Two queries, two surfaces

The omnibox and the library want opposite things from the same table, so they
use different queries and neither is a special case of the other.

`search_history` ranks by frecency and deduplicates by address: the launcher
offers destinations, not a log. Capacity there is counted per kind rather than
per section, because History mixes visited pages with previously submitted
queries and a shared cap let the queries crowd the pages out entirely.

`history_page` returns every visit, newest first, repeats included. It pages on
`history.id` rather than `visited_at`: ids rise with insertion, so a page
boundary is exact even when dozens of visits share one second, and the primary
key already provides that order. An empty query browses the table directly; a
non-empty one joins the same FTS index the omnibox uses.

## Titles

A visit is recorded when its URL commits. On a cross-origin navigation the shell
deliberately replaces the title with a neutral URL-derived label until the
document publishes its own, so the recorded row held that placeholder and the
real title never reached history — most entries read as bare hostnames.

`TitleChanged` now amends the newest visit to that exact address, bounded to
sixty seconds so a title arriving after the reader has moved on cannot rewrite
an older visit. The FTS and byte-ledger triggers keep the index and the quota
correct across the update.

## Deleting

Forgetting an entry removes every visit to that address: in a history list the
reader means the page, not one of the times they opened it. Clearing takes a
range and also drops recorded searches in it. Both run on the store reader
thread, never on the shell actor.

Settings › Privacy clears history for real. Cookies and website cache still need
the engine data store and remain labelled as preview.

## Windowing

Nothing else in the frame virtualises a list; `ResourcePanel` uses a More button
and a thousand-row cap. History scrolls years, so `HistoryList` renders a window.

Day headings and visits have two known heights, so offsets are a prefix sum
computed once per appended page and the row under the scroll position is a
binary search. Below two hundred rows the list renders plainly — the machinery
would buy nothing at that size and costs a spacer pair. Five thousand visits are
held in memory; past that the reader narrows the search instead.

## Site icons

Chrome names icons rather than carrying them. `TabView` and `SearchResult` hold
an `IconRef` — origin plus a content revision — and rasters travel on their own
projection, sent once per surface.

Measured on the real wire types: a thirty-tab items projection was 170,781 bytes
and is 8,531; the rasters that back it are 166,263 bytes sent once rather than
on every tab change.

References drive delivery. Naming an icon whose pixels a surface does not hold
at that revision queues them, and they are emitted immediately before the
projection that names them, so a reference is never ahead of its pixels. Chrome
and the launcher panel are tracked separately, so opening the launcher does not
resend the sidebar's icons. A reattached surface — a chrome reload, a freshly
opened panel — forgets what it was sent.

The frame's cache holds at least as many origins as the native one. Native sends
only what it believes a surface lacks, so evicting inside its bound would strand
an origin whose pixels are never resent.

### Why icons went missing

Three independent causes, all fixed:

- Age gated display rather than refresh. A raster older than a week made the
  read return nothing, so the shell fell through to renderer discovery — which a
  restored tab, having no view, can never complete. A stored raster is now drawn
  whatever its age, and refreshed in parallel.
- Hydration covered one section of one space. It now walks every section the
  sidebar projects, folder contents included.
- History and search rows read a tab-only cache, so a row got an icon only if
  that site happened to be open. They resolve through the same cache the sidebar
  fills.

Stored favicon retention is 1024 origins per profile, 4 MiB at the fixed 4 KiB
raster size. A page whose icon differs from its origin's also keeps its own, by
address without query or fragment: 512 per profile, 2 MiB. They are history, so
clearing or forgetting history removes them, and a private page never stores
one. The origin's icon stays the fallback for a page that declares none and for
a listed address not yet opened.

## Opening a row

The library and the launcher panel have no tab id and must not be handed one.
`browser_open_url` opens an address in the focused window; in place it goes
through the ordinary navigation path, which returns from the browser page first,
so a row lands in the tab the reader was already looking at.

## Lifecycle details worth keeping

The session registry is reactive and starting a session reads its own state, so
a tracked effect body restarted the session on every change it made — including
the query just typed. The body is untracked, with the owning profile as its only
dependency.

A reopened surface shows an empty field, so stopping a session clears its query
rather than leaving the list filtered by a stale one. The panel seeds its first
request from the query its frame already holds, so a persisted search never
flashes the unfiltered list first.

## Not verified

No native application run was available. Every visual result here is a WebKit
component screenshot; nothing is claimed about the real shell's material, focus
behaviour, IME, or other platforms. Cookie and cache clearing are not
implemented.
