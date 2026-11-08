import { getCurrentWindow } from "@tauri-apps/api/window";
import { mount, flushSync } from "svelte";
import App from "$app/little/LittleApp.svelte";

try {
  const label = getCurrentWindow().label;
  if (!/^little-[1-9]\d*$/.test(label)) throw new Error("unexpected native surface");
  const target = document.getElementById("root");
  if (!(target instanceof HTMLElement)) throw new Error("trusted UI root is unavailable");
  mount(App, { target, props: { label } });
  // The native presentation barrier observes this DOM synchronously.
  flushSync();
} catch {
  console.error("trusted Little window initialization failed");
}
