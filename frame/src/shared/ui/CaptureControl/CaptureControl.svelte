<script lang="ts">
  import { Camera01Icon, Mic01Icon } from "@hugeicons/core-free-icons";
  import * as m from "$shared/i18n/messages";
  import Icon from "$shared/ui/Icon";
  type CaptureDisplayState = {
    camera: "none" | "active" | "muted";
    microphone: "none" | "active" | "muted";
  };

  let {
    site,
    capture,
    onStop,
  }: { site: string; capture: CaptureDisplayState; onStop: () => Promise<boolean> } = $props();
  let busy = $state(false);
  let failed = $state(false);
  let camera = $derived(capture.camera !== "none");
  let microphone = $derived(capture.microphone !== "none");
  let title = $derived(
    [
      camera
        ? capture.camera === "muted"
          ? m.page_capture_camera_muted()
          : m.page_capture_camera()
        : "",
      microphone
        ? capture.microphone === "muted"
          ? m.page_capture_microphone_muted()
          : m.page_capture_microphone()
        : "",
    ]
      .filter(Boolean)
      .join(", "),
  );

  async function stop(event: MouseEvent) {
    event.stopPropagation();
    if (busy) return;
    busy = true;
    failed = false;
    try {
      failed = !(await onStop());
    } catch {
      failed = true;
    } finally {
      busy = false;
    }
  }
</script>

<span class="capture-control">
  <button
    type="button"
    class="capture-stop"
    title={failed ? m.page_capture_stop_failed() : title}
    aria-label={m.page_capture_stop({ site })}
    aria-busy={busy || undefined}
    disabled={busy}
    onclick={stop}
    data-zephium-capture-control
  >
    <Icon icon={camera ? Camera01Icon : Mic01Icon} size={14} />
    <span class="sr-only">{title}</span>
  </button>
  {#if failed}<span role="alert" class="sr-only">{m.page_capture_stop_failed()}</span>{/if}
</span>

<style>
  .capture-control {
    display: inline-flex;
    align-items: center;
    flex: 0 0 auto;
  }

  .capture-stop {
    display: grid;
    place-items: center;
    inline-size: 24px;
    block-size: 24px;
    border: 0;
    border-radius: var(--radius-control-compact);
    background: transparent;
    color: var(--color-danger);
    cursor: pointer;
  }

  .capture-stop:hover {
    background: var(--row-hover);
  }

  .capture-stop:focus-visible {
    outline: 2px solid var(--color-accent);
    outline-offset: 1px;
  }

  .capture-stop:disabled {
    opacity: 0.6;
    cursor: progress;
  }
</style>
