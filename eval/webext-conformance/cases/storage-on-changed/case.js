export async function run(h) {
  for (const name of ["local", "sync", "session"]) {
    const area = chrome.storage[name];
    await area.clear();
    for (const [operation, execute] of [
      ["insert", () => area.set({ alpha: 1, nested: { items: [true, null] } })],
      ["replace", () => area.set({ alpha: 2 })],
      ["remove", () => area.remove("alpha")],
      ["clear", () => area.clear()],
    ]) {
      const changed = h.event(
        chrome.storage.onChanged,
        (changes, areaName) => areaName === name,
      );
      h.record(`${name}.${operation}.call`, {});
      await execute();
      const [changes, areaName] = await changed;
      h.record(`${name}.${operation}.onChanged`, { areaName, changes });
    }
  }
}
