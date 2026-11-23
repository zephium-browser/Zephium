import { preferences } from "$domain/preferences";
import { tabs } from "$domain/tabs";

// Only a failed local display is session state. The choice stays in Rust.
let failed = $state("");
export function newTabImage() {
  if (preferences.value("ui.newtab-background") !== "image") return null;
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
