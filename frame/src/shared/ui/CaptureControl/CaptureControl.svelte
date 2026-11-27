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
    badge = false,
    onOpen,
  }: {
    site: string;
    capture: CaptureDisplayState;
    /** A small raised disc on a mark's corner, where there is no row. */
    badge?: boolean;
    /** Opens what can be done about it, anchored under the indicator. */
    onOpen: (x: number, y: number) => void;
  } = $props();
  let camera = $derived(capture.camera !== "none");
  let microphone = $derived(capture.microphone !== "none");
  // Recording is red, as Chrome and Safari draw it; a muted device is not
  // recording and steps back.
  let live = $derived(capture.camera === "active" || capture.microphone === "active");
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

  function open(event: MouseEvent) {
    event.stopPropagation();
    const anchor = (event.currentTarget as HTMLElement).getBoundingClientRect();
    onOpen(anchor.left, anchor.bottom);
  }
</script>

<button
  type="button"
  class="capture-control"
  data-live={live || undefined}
  data-badge={badge || undefined}
  {title}
  aria-label={m.page_capture_menu({ site })}
  aria-haspopup="menu"
  onclick={open}
  onpointerdown={(event) => event.stopPropagation()}
  data-zephium-capture-control
>
  <Icon icon={camera ? Camera01Icon : Mic01Icon} size={badge ? 10 : 13} />
  <span class="sr-only">{title}</span>
</button>

<style>
  .capture-control {
    display: grid;
    flex: 0 0 auto;
    place-items: center;
    inline-size: 22px;
    block-size: 22px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-capsule);
    background: transparent;
    color: var(--color-faint);
    cursor: default;
    transition: background-color var(--motion-fast) var(--ease-out);
  }

  .capture-control[data-live] {
    background: color-mix(in srgb, var(--color-danger) 18%, transparent);
    color: var(--color-danger);
  }

  .capture-control[data-badge] {
    inline-size: 16px;
    block-size: 16px;
    background: var(--color-float);
    box-shadow: var(--shadow-badge);
  }

  .capture-control[data-badge][data-live] {
    background: var(--color-danger);
    color: var(--color-float);
  }

  .capture-control:hover {
    background: var(--row-pressed);
  }

  .capture-control[data-live]:hover {
    background: color-mix(in srgb, var(--color-danger) 28%, transparent);
  }

  .capture-control:focus-visible {
    outline: 2px solid var(--color-accent);
    outline-offset: 1px;
  }

  @media (forced-colors: active) {
    .capture-control[data-live] {
      color: Mark;
    }
  }
</style>
