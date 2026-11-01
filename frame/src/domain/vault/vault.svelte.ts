/**
 * The person's vault as native reports it: which folder it is and the
 * listings the explorer has asked for. Rust owns every path decision; this is
 * a projection plus typed intents, refreshed by `vault-changed` events.
 */
import {
  commands,
  type NoteSummary,
  type VaultCall,
  type VaultEntry,
  type VaultError,
  type VaultResponse,
} from "$shared/ipc/bindings";
import { SvelteMap } from "svelte/reactivity";
import { events } from "$shared/ipc/native-events";
import { IS_MAC } from "$shared/platform";

const FILES = IS_MAC ? "zephium-vault://localhost/" : "http://zephium-vault.localhost/";

export type { VaultEntry, VaultError, VaultResponse };

export type Outcome<T> = { ok: true; value: T } | { ok: false; error: VaultError };

export class VaultSession {
  readonly profile: string;
  /** `undefined` while asking native, `null` when no vault is chosen. */
  root = $state<string | null | undefined>(undefined);
  /** Folder path ("" is the root) to its listing. */
  readonly listings = new SvelteMap<string, readonly VaultEntry[]>();
  readonly failed = new SvelteMap<string, VaultError>();
  /** Bumped on every vault change, for views that read files. */
  revision = $state(0);
  #stop: (() => void) | null = null;
  #notes: Record<string, string> | null = null;
  #stopNotes: (() => void) | null = null;
  #alive = true;

  constructor(profile: string) {
    this.profile = profile;
  }

  async start() {
    const [stop, stopNotes] = await Promise.all([
      events.vaultChanged.listen(({ payload }) => {
        if (payload.profile !== this.profile) return;
        this.revision += 1;
        void this.refresh(payload.dirs.length === 0 ? null : payload.dirs);
      }),
      events.notesChanged.listen(() => {
        this.#notes = null;
      }),
    ]);
    if (!this.#alive) {
      stop();
      stopNotes();
      return;
    }
    this.#stop = stop;
    this.#stopNotes = stopNotes;
    await this.loadRoot();
  }

  stop() {
    this.#alive = false;
    this.#stop?.();
    this.#stopNotes?.();
    this.#stop = null;
    this.#stopNotes = null;
  }

  async loadRoot() {
    try {
      this.root = await commands.vaultRoot(this.profile);
    } catch {
      this.root = null;
    }
    if (this.root) await this.list("");
  }

  async chooseRoot(): Promise<Outcome<string | null>> {
    try {
      const result = await commands.vaultChooseRoot(this.profile);
      if (result.status === "error") return { ok: false, error: result.error };
      if (result.data !== null) {
        this.listings.clear();
        this.#notes = null;
        this.root = result.data;
        await this.list("");
      }
      return { ok: true, value: result.data };
    } catch {
      return { ok: false, error: "unavailable" };
    }
  }

  async call(call: VaultCall): Promise<VaultResponse> {
    try {
      return await commands.vaultCall(this.profile, call);
    } catch {
      return { kind: "error", error: "unavailable" };
    }
  }

  /** Runs a call that answers with one entry, then refreshes the folders it touched. */
  async change(call: VaultCall, touched: string[]): Promise<Outcome<VaultEntry | null>> {
    const response = await this.call(call);
    if (response.kind === "error") return { ok: false, error: response.error };
    this.revision += 1;
    await this.refresh(touched);
    return { ok: true, value: response.kind === "entry" ? response.entry : null };
  }

  async list(dir: string): Promise<readonly VaultEntry[] | null> {
    const response = await this.call({ kind: "list", dir });
    if (response.kind === "entries") {
      this.listings.set(dir, response.entries);
      this.failed.delete(dir);
      return response.entries;
    }
    if (response.kind === "error") {
      this.failed.set(dir, response.error);
      if (response.error === "not_found") this.listings.delete(dir);
    }
    return null;
  }

  /** Reloads the named folders that are already listed, or all of them. */
  async refresh(dirs: string[] | null) {
    const loaded = [...this.listings.keys()];
    const wanted = dirs === null ? loaded : dirs.filter((dir) => this.listings.has(dir));
    await Promise.all(wanted.map((dir) => this.list(dir)));
  }

  async search(query: string, limit = 50, foldersOnly = false): Promise<VaultEntry[]> {
    const response = await this.call({ kind: "search", query, limit, folders_only: foldersOnly });
    return response.kind === "entries" ? response.entries : [];
  }

  async readText(path: string): Promise<Outcome<{ text: string; modified: string }>> {
    const response = await this.call({ kind: "read", path });
    if (response.kind === "error") return { ok: false, error: response.error };
    if (response.kind !== "bytes") return { ok: false, error: "io" };
    const bytes = fromBase64(response.base64);
    return {
      ok: true,
      value: { text: new TextDecoder().decode(bytes), modified: response.modified },
    };
  }

  /** The URL a viewer fetches a vault file's bytes from. */
  url(path: string): string {
    return FILES + encodeURIComponent(`${this.profile}/${path}`);
  }

  /** The note identity of a Markdown file in the vault, if notes indexed it. */
  async noteId(path: string): Promise<string | null> {
    if (!this.#notes) {
      const map: Record<string, string> = Object.create(null) as Record<string, string>;
      let after: string | null = null;
      for (let page = 0; page < 100; page++) {
        let reply;
        try {
          reply = await commands.noteCall(this.profile, {
            kind: "list",
            query: { search: "", trashed: false, after, limit: 200 },
          });
        } catch {
          break;
        }
        if (reply.response.kind !== "page") break;
        for (const note of reply.response.items as NoteSummary[]) map[note.path] = note.id;
        after = reply.response.next;
        if (!after) break;
      }
      this.#notes = map;
    }
    return this.#notes[path] ?? null;
  }
}

function fromBase64(text: string): Uint8Array {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

export function toBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let index = 0; index < bytes.length; index += chunk) {
    binary += String.fromCharCode(...bytes.subarray(index, index + chunk));
  }
  return btoa(binary);
}

/** The folder that contains a vault path ("" for the root). */
export function parentOf(path: string): string {
  const at = path.lastIndexOf("/");
  return at < 0 ? "" : path.slice(0, at);
}

export function nameOf(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}
