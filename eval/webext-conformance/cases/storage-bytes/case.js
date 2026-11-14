export async function run(h) {
  for (const name of ["local", "sync", "session"]) {
    const area = chrome.storage[name];
    await area.clear();
    await h.call(`${name}.empty`, () => area.getBytesInUse(null));
    await area.set({
      ascii: "a",
      unicode: "λ😀",
      object: { count: 3 },
      empty: "",
    });
    await h.call(`${name}.all`, () => area.getBytesInUse(null));
    await h.call(`${name}.one`, () => area.getBytesInUse("unicode"));
    await h.call(`${name}.selected`, () =>
      area.getBytesInUse(["ascii", "object", "missing"]),
    );
    await h.callback(`${name}.omitted.callback`, (done) =>
      area.getBytesInUse(done),
    );
    await h.call(`${name}.missing`, () => area.getBytesInUse("missing"));
    await area.remove("unicode");
    await h.call(`${name}.afterRemove`, () => area.getBytesInUse(null));
    await area.clear();
  }
}
