import { getCurrentWindow } from "@tauri-apps/api/window";
import { mount, flushSync } from "svelte";
import App from "$app/panel/PanelApp.svelte";
import { IS_LINUX } from "$shared/platform";

try {
  if (getCurrentWindow().label !== "panel") throw new Error("unexpected native surface");
  const target = document.getElementById("root");
  if (!(target instanceof HTMLElement)) throw new Error("trusted UI root is unavailable");
  if (IS_LINUX) document.documentElement.dataset.platform = "linux";
  mount(App, { target });
  // The native presentation barrier observes this DOM synchronously.
  flushSync();
} catch {
  console.error("trusted panel initialization failed");
}
