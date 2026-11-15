export async function run(h) {
  const probes = Array.from({length: 16}, async (_, index) => {
    const port = chrome.runtime.connect({name: `ready-${index}`});
    const [ready] = await h.event(port.onMessage);
    h.assert(ready.ready === true && ready.name === `ready-${index}`, "first message delivered");
    const echoed = h.event(port.onMessage);
    port.postMessage(index);
    h.assert((await echoed)[0].echo === index, "same port responds");
    port.disconnect();
    return index;
  });
  h.record("firstMessages", await Promise.all(probes));
}
