/**
 * Find in page. The address field becomes the search while it runs: the page
 * it would describe is the page being searched. Native highlights and counts;
 * this holds what the field shows and answers only for the newest text.
 */
import { commands } from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { expandBriefly } from "$session/sidebar-mode.svelte";

const TYPING_PAUSE_MS = 90;

let open = $state(false);
/** The tab being searched; moving to another one ends the search. */
let page: string | null = null;
let query = $state("");
let matches = $state<number | null>(null);
let active = $state<number | null>(null);
/** Raised each time the field should take focus, read by whichever field is mounted. */
let focusRequest = $state(0);
/** Kept after the field closes, so Find Next can pick up where it left off. */
let last = "";
let timer: ReturnType<typeof setTimeout> | undefined;
let restoreSidebar: (() => void) | null = null;
let stop: (() => void) | null = null;
let disposed = false;
let request = 0;

export const isOpen = () => open;
export const searching = () => page;
export const text = () => query;
export const count = () => matches;
export const position = () => active;
export const focusRequested = () => focusRequest;

function listen() {
  if (stop) return;
  disposed = false;
  void events.find
    .listen(({ payload }) => {
      if (!open || payload.query !== query) return;
      matches = payload.matches;
      active = payload.active;
    })
    .then((off) => {
      if (disposed) off();
      else stop = off;
    });
}

function search(value: string, forward: boolean) {
  const attempt = ++request;
  const failed = () => {
    if (!open || query !== value || request !== attempt) return;
    matches = 0;
    active = null;
  };
  void commands
    .pageFind(value, forward)
    .then((accepted) => {
      if (!accepted) failed();
    })
    .catch(failed);
}

/** Opens the field (revealing a compact sidebar for as long as it is open)
 *  and asks for focus. */
export function show(tab: string | null) {
  listen();
  page = tab;
  if (!open) {
    open = true;
    restoreSidebar = expandBriefly();
  }
  focusRequest += 1;
}

export function setText(value: string) {
  request += 1;
  query = value.slice(0, 512);
  matches = null;
  active = null;
  clearTimeout(timer);
  if (!query) {
    void commands.pageFind(null, true).catch(() => false);
    return;
  }
  last = query;
  const wanted = query;
  timer = setTimeout(() => search(wanted, true), TYPING_PAUSE_MS);
}

/** The next or previous match. With the field closed, reopens it on the last
 *  search, the way Find Next works in every browser. */
export function step(forward: boolean, tab: string | null) {
  if (!open) {
    show(tab);
    if (!last) return;
    query = last;
  }
  if (!query) return;
  clearTimeout(timer);
  search(query, forward);
}

export function hide() {
  if (!open) return;
  open = false;
  request += 1;
  page = null;
  clearTimeout(timer);
  query = "";
  matches = null;
  active = null;
  void commands.pageFind(null, true).catch(() => false);
  restoreSidebar?.();
  restoreSidebar = null;
}

export function dispose() {
  hide();
  disposed = true;
  stop?.();
  stop = null;
}
