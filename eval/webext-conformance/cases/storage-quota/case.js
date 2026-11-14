export async function run(h) {
  for (const name of ["local", "sync", "session"])
    await chrome.storage[name].clear();
  h.record("quota.constants", {
    local: chrome.storage.local.QUOTA_BYTES,
    sync: chrome.storage.sync.QUOTA_BYTES,
    syncPerItem: chrome.storage.sync.QUOTA_BYTES_PER_ITEM,
    session: chrome.storage.session.QUOTA_BYTES,
  });
  await h.call("local.overTotal.promise", () =>
    chrome.storage.local.set({
      oversized: "x".repeat(chrome.storage.local.QUOTA_BYTES + 1),
    }),
  );
  await h.callback("sync.overItem.callback", (done) =>
    chrome.storage.sync.set(
      {
        oversized: "x".repeat(chrome.storage.sync.QUOTA_BYTES_PER_ITEM + 1),
      },
      done,
    ),
  );
  const many = Object.fromEntries(
    Array.from({ length: 26 }, (_, index) => [
      `item${index}`,
      "x".repeat(4096),
    ]),
  );
  await h.call("sync.overTotal.promise", () => chrome.storage.sync.set(many));
  await h.call("session.overTotal.promise", () =>
    chrome.storage.session.set({
      oversized: "x".repeat(chrome.storage.session.QUOTA_BYTES + 1),
    }),
  );
  for (const name of ["local", "sync", "session"]) {
    await h.call(`${name}.afterRejectedWrites`, () =>
      chrome.storage[name].get(null),
    );
    await chrome.storage[name].clear();
  }
}
