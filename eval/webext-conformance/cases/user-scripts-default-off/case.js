export async function run(h) {
  h.record("namespace", {type: typeof chrome.userScripts, own: Object.hasOwn(chrome, "userScripts")});
  h.assert(chrome.userScripts === undefined, "default-off userScripts must be unavailable");
}
