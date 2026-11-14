export async function run(h) {
  await chrome.storage.session.clear();
  await chrome.storage.session.set({
    sessionProbe: "visible-only-when-authorized",
  });
  await chrome.storage.session.setAccessLevel({
    accessLevel: "TRUSTED_CONTEXTS",
  });
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  try {
    await h.call("content.defaultAccess", () =>
      chrome.tabs.sendMessage(tab.id, { probe: "session-access" }),
    );
    await h.call("session.allowUntrusted", () =>
      chrome.storage.session.setAccessLevel({
        accessLevel: "TRUSTED_AND_UNTRUSTED_CONTEXTS",
      }),
    );
    await h.call("content.grantedAccess", () =>
      chrome.tabs.sendMessage(tab.id, { probe: "session-access" }),
    );
    await h.call("session.revokeUntrusted", () =>
      chrome.storage.session.setAccessLevel({
        accessLevel: "TRUSTED_CONTEXTS",
      }),
    );
    await h.call("content.revokedAccess", () =>
      chrome.tabs.sendMessage(tab.id, { probe: "session-access" }),
    );
    await h.call("session.invalidAccessLevel", () =>
      chrome.storage.session.setAccessLevel({ accessLevel: "EVERYONE" }),
    );
  } finally {
    await chrome.tabs.remove(tab.id);
    await chrome.storage.session.clear();
  }
}
