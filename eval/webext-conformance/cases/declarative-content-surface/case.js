export async function run(h) {
  for (const name of ["PageStateMatcher", "ShowAction", "ShowPageAction", "SetIcon", "RequestContentScript"]) {
    h.record(name, typeof chrome.declarativeContent[name]);
  }
}
