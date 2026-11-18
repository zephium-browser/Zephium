export async function run(h) {
  await h.call("get all permissions", () => chrome.permissions.getAll());
  await h.call("contains granted storage", () =>
    chrome.permissions.contains({ permissions: ["storage"] }),
  );
  await h.call("contains ungranted alarms", () =>
    chrome.permissions.contains({ permissions: ["alarms"] }),
  );
  await h.call("contains collector origin", () =>
    chrome.permissions.contains({ origins: ["http://127.0.0.1/*"] }),
  );
  await h.call("contains optional origin", () =>
    chrome.permissions.contains({ origins: ["http://localhost/*"] }),
  );
  await h.callback("contains callback", (callback) =>
    chrome.permissions.contains({ permissions: ["storage"] }, callback),
  );
  await h.call("contains unknown permission", () =>
    chrome.permissions.contains({
      permissions: ["conformanceUnknownPermission"],
    }),
  );
}
