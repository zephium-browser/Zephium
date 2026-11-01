<!--
  A Word document, rendered read-only with docx-preview inside a shadow root
  so its page styles cannot leak into the chrome. Edit it in Word.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import type { VaultSession } from "$domain/vault";
  import * as m from "$shared/i18n/messages";
  import NativeBar from "./NativeBar.svelte";

  let { session, path }: { session: VaultSession; path: string } = $props();
  let host = $state<HTMLDivElement | null>(null);
  let status = $state<"loading" | "ready" | "failed">("loading");

  onMount(() => {
    let alive = true;
    void (async () => {
      try {
        const [{ renderAsync }, response] = await Promise.all([
          import("docx-preview"),
          fetch(session.url(path)),
        ]);
        if (!response.ok) throw new Error(String(response.status));
        const data = await response.arrayBuffer();
        if (!alive || !host) return;
        const root = host.attachShadow({ mode: "open" });
        const styles = document.createElement("div");
        const body = document.createElement("div");
        root.append(styles, body);
        await renderAsync(data, body, styles, {
          inWrapper: true,
          ignoreLastRenderedPageBreak: true,
          experimental: false,
          useBase64URL: true,
          renderComments: false,
        });
        status = "ready";
      } catch {
        if (alive) status = "failed";
      }
    })();
    return () => {
      alive = false;
    };
  });
</script>

<div class="docx">
  <NativeBar {session} {path} />
  <div class="pages" bind:this={host}></div>
  {#if status === "loading"}<p>{m.surface_loading()}</p>{/if}
  {#if status === "failed"}<p role="alert">{m.vault_open_failed()}</p>{/if}
</div>

<style>
  .docx {
    display: flex;
    flex-direction: column;
    block-size: 100%;
  }

  .pages {
    flex: 1;
    min-block-size: 0;
    overflow: auto;
    background: var(--color-canvas);
  }

  p {
    padding: 16px;
    color: var(--color-muted);
  }
</style>
