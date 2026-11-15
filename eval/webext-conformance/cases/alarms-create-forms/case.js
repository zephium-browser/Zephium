function describe(alarm) {
  if (!alarm) return null;
  return {
    name: alarm.name,
    scheduled: alarm.scheduledTime > Date.now() ? "future" : "due",
    periodInMinutes: alarm.periodInMinutes ?? null,
    persistAcrossSessions: alarm.persistAcrossSessions ?? null,
  };
}

export async function run(h) {
  await chrome.alarms.clearAll();
  await h.callback("create.named.callback", (done) =>
    chrome.alarms.create(
      "named",
      { delayInMinutes: 2, periodInMinutes: 3 },
      done,
    ),
  );
  await h.call("create.unnamed.promise", () =>
    chrome.alarms.create({ when: Date.now() + 120000 }),
  );
  await h.call("get.named", async () =>
    describe(await chrome.alarms.get("named")),
  );
  await h.call("get.unnamed", async () => describe(await chrome.alarms.get()));
  await h.call("get.missing", async () =>
    describe(await chrome.alarms.get("absent")),
  );
  await h.call("getAll", async () =>
    (await chrome.alarms.getAll())
      .map(describe)
      .sort((a, b) => a.name.localeCompare(b.name)),
  );
  await h.call("replace.named", () =>
    chrome.alarms.create("named", { delayInMinutes: 4 }),
  );
  await h.call("get.replaced", async () =>
    describe(await chrome.alarms.get("named")),
  );
  await h.call("create.conflictingSchedule", () =>
    chrome.alarms.create("invalid", {
      when: Date.now() + 120000,
      delayInMinutes: 2,
    }),
  );
  await h.callback("clear.named.callback", (done) =>
    chrome.alarms.clear("named", done),
  );
  await h.call("clear.missing.promise", () => chrome.alarms.clear("absent"));
  await h.call("clear.unnamed.promise", () => chrome.alarms.clear());
  await h.call("clearAll.promise", () => chrome.alarms.clearAll());
  await h.call("getAll.afterClear", () => chrome.alarms.getAll());
}
