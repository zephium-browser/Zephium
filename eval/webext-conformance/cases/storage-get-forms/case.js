export async function run(h) {
  for (const name of ["local", "sync", "session"]) {
    const area = chrome.storage[name];
    await area.clear();
    await area.set({ alpha: "A", count: 7, nested: { enabled: true } });
    await h.call(`${name}.get.omitted`, () => area.get());
    await h.call(`${name}.get.null`, () => area.get(null));
    await h.call(`${name}.get.string`, () => area.get("alpha"));
    await h.call(`${name}.get.array`, () => area.get(["count", "missing"]));
    await h.call(`${name}.get.defaults`, () =>
      area.get({ alpha: "fallback", missing: 42 }),
    );
    await h.callback(`${name}.get.callback`, (done) =>
      area.get({ nested: null, missing: false }, done),
    );
    await area.clear();
  }
  await h.call("managed.get.null", () => chrome.storage.managed.get(null));
  await h.callback("managed.get.callback", (done) =>
    chrome.storage.managed.get({ missing: "default" }, done),
  );
}
