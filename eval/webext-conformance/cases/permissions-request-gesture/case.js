export async function run(h) {
  h.record("user activation", {
    isActive: navigator.userActivation.isActive,
    hasBeenActive: navigator.userActivation.hasBeenActive,
  });
  await h.call("request alarms without gesture", () =>
    chrome.permissions.request({ permissions: ["alarms"] }),
  );
  await h.callback(
    "request optional origin callback without gesture",
    (callback) =>
      chrome.permissions.request({ origins: ["http://localhost/*"] }, callback),
  );
  await h.call("permissions remain absent", () =>
    chrome.permissions.contains({
      permissions: ["alarms"],
      origins: ["http://localhost/*"],
    }),
  );
}
