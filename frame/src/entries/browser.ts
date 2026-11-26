import { getCurrentWindow } from "@tauri-apps/api/window";
import { mount, flushSync } from "svelte";
import App from "$app/browser/BrowserApp.svelte";

try {
  const label = getCurrentWindow().label;
  if (label !== "main" && !/^browser-[1-9]\d{0,19}$/.test(label)) {
    throw new Error("unexpected native surface");
  }
  const target = document.getElementById("root");
  if (!(target instanceof HTMLElement)) throw new Error("trusted UI root is unavailable");
  mount(App, { target });
  // The native presentation barrier observes this DOM synchronously.
  flushSync();
} catch (error) {
  console.error("trusted browser initialization failed", error);
}
