export async function run(h) {
  await chrome.alarms.clearAll();
  for (const [name, info] of [
    ["below-minimum", { delayInMinutes: 0.1, periodInMinutes: 0.1 }],
    ["at-minimum", { delayInMinutes: 0.5, periodInMinutes: 0.5 }],
    ["below-one-second", { delayInMinutes: 0.1, periodInMinutes: 0.001 }],
    ["negative-period", { delayInMinutes: 2, periodInMinutes: -1 }],
  ]) {
    await h.call(`${name}.create`, () => chrome.alarms.create(name, info));
    await h.call(`${name}.get`, async () => {
      const alarm = await chrome.alarms.get(name);
      return alarm
        ? {
            name: alarm.name,
            periodInMinutes: alarm.periodInMinutes ?? null,
            scheduled: alarm.scheduledTime > Date.now() ? "future" : "due",
          }
        : null;
    });
    await chrome.alarms.clear(name);
  }
  await h.sleep(0);
  const messages = await h.host("consoleMessages", {});
  const diagnostics = [
    ...new Set(
      messages
        .map((entry) => (typeof entry === "string" ? entry : entry.text))
        .filter((text) => /alarm|minimum|periodInMinutes/i.test(text)),
    ),
  ];
  h.record("console.alarmDiagnostics", diagnostics);
}
