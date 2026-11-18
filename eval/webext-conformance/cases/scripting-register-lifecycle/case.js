export async function run(h) {
  const before = await h.openTab(
    `${h.origins.a}/page.html?before-registration`,
  );
  const script = {
    id: "probe",
    matches: ["http://127.0.0.1/*"],
    js: ["registered-a.js"],
    runAt: "document_start",
    persistAcrossSessions: false,
  };
  await h.call("register", () =>
    chrome.scripting.registerContentScripts([script]),
  );
  await h.call("registered", () =>
    chrome.scripting.getRegisteredContentScripts({ ids: ["probe"] }),
  );
  await h.call("duplicate id", () =>
    chrome.scripting.registerContentScripts([script]),
  );
  const read = (tabId) =>
    chrome.scripting.executeScript({
      target: { tabId },
      func: () => document.documentElement.dataset.registered ?? null,
    });
  await h.call("existing document is unchanged", () => read(before.id));
  const first = await h.openTab(`${h.origins.a}/page.html?registered-a`);
  await h.call("new document uses registered script", () => read(first.id));
  await h.call("update", () =>
    chrome.scripting.updateContentScripts([
      { id: "probe", js: ["registered-b.js"] },
    ]),
  );
  await h.call("updated registration", () =>
    chrome.scripting.getRegisteredContentScripts(),
  );
  const second = await h.openTab(`${h.origins.a}/page.html?registered-b`);
  await h.call("new document uses updated script", () => read(second.id));
  await h.call("unregister", () =>
    chrome.scripting.unregisterContentScripts({ ids: ["probe"] }),
  );
  await h.call("empty registrations", () =>
    chrome.scripting.getRegisteredContentScripts(),
  );
  await h.call("unregistration preserves existing injection", () =>
    read(second.id),
  );
  const after = await h.openTab(`${h.origins.a}/page.html?after-unregister`);
  await h.call("later document has no injection", () => read(after.id));
  await h.call("update missing id", () =>
    chrome.scripting.updateContentScripts([
      { id: "missing", js: ["registered-a.js"] },
    ]),
  );
}
