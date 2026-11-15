export async function run(h) {
  for (const name of ["get", "getAll", "setEnabled", "uninstall", "getSelf", "uninstallSelf", "getPermissionWarningsByManifest"]) {
    h.record(name, typeof chrome.management[name]);
  }
  for (const name of ["onInstalled", "onUninstalled", "onEnabled", "onDisabled"]) {
    h.record(name, typeof chrome.management[name]?.addListener);
  }
}
