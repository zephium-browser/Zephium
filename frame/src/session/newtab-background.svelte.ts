import { preferences } from "$domain/preferences";
import { tabs } from "$domain/tabs";

export type NewTabGround = "sky" | "image" | "plain";

/** What the new tab is drawn on: the person's choice, else plain under Home and the sky without it. */
export function newTabBackground(): NewTabGround {
  const stored = preferences.chosen("ui.newtab-background")
    ? preferences.value("ui.newtab-background")
    : preferences.backgroundDefault(tabs.homeOn());
  return stored === "image" || stored === "plain" ? stored : "sky";
}

// Only a failed local display is session state. The choice stays in Rust.
let failed = $state("");
export function newTabImage() {
  if (newTabBackground() !== "image") return null;
  const digest = preferences.value("ui.newtab-image");
  const profile = tabs.profile()?.id;
  if (!profile || !/^[0-9a-f]{64}$/u.test(digest)) return null;
  const key = `${profile}/${digest}`;
  return failed === key ? null : { profile, digest, key };
}
export function imageFailed(key: string) {
  failed = key;
}
export function retryImage() {
  failed = "";
}
