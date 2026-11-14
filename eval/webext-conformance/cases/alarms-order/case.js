export async function run(h) {
  await chrome.alarms.clearAll();
  for (const name of ["first", "second"]) {
    const fired = h.event(
      chrome.alarms.onAlarm,
      (alarm) => alarm.name === name,
    );
    h.record("alarm.create", { name, when: "past" });
    await chrome.alarms.create(name, { when: Date.now() - 1000 });
    const [alarm] = await fired;
    h.record("alarm.onAlarm", {
      name: alarm.name,
      scheduled: alarm.scheduledTime <= Date.now() ? "due" : "future",
      periodInMinutes: alarm.periodInMinutes ?? null,
    });
    await h.call(
      `${name}.getAfterFire`,
      async () => (await chrome.alarms.get(name)) ?? null,
    );
  }
  await chrome.alarms.create("replacement", {
    delayInMinutes: 2,
    periodInMinutes: 3,
  });
  await chrome.alarms.create("replacement", { delayInMinutes: 4 });
  await h.call("replacement.get", async () => {
    const alarm = await chrome.alarms.get("replacement");
    return {
      name: alarm.name,
      periodInMinutes: alarm.periodInMinutes ?? null,
      scheduled: "future",
    };
  });
  await h.call("replacement.clear", () => chrome.alarms.clear("replacement"));
  await h.call("getAll.final", () => chrome.alarms.getAll());
}
