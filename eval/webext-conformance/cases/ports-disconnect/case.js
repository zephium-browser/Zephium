export async function run(h) {
  const listener = (message, sender, respond) => {
    if (message.probe === "remote-disconnect") respond({ acknowledged: true });
  };
  chrome.runtime.onMessage.addListener(listener);
  try {
    const client = chrome.runtime.connect({ name: "client-close" });
    const [ready] = await h.event(client.onMessage);
    h.record("client.ready", ready);
    const echoed = h.event(client.onMessage);
    client.postMessage({ sequence: 1, payload: ["hello", null] });
    h.record("client.echo", (await echoed)[0]);
    let ownDisconnects = 0;
    client.onDisconnect.addListener(() => {
      ownDisconnects += 1;
    });
    const remote = h.event(
      chrome.runtime.onMessage,
      (message) => message.probe === "remote-disconnect",
    );
    client.disconnect();
    const [remoteMessage] = await remote;
    h.record("client.disconnect.remoteEvent", remoteMessage);
    h.record("client.disconnect.localEventCount", ownDisconnects);
    await h.call("client.postAfterDisconnect", () =>
      client.postMessage({ sequence: 2 }),
    );

    const peer = chrome.runtime.connect({ name: "peer-close" });
    h.record("peer.ready", (await h.event(peer.onMessage))[0]);
    const closed = new Promise((resolve) =>
      peer.onDisconnect.addListener((port) =>
        resolve({
          samePort: port === peer,
          lastError: chrome.runtime.lastError?.message ?? null,
        }),
      ),
    );
    peer.postMessage({ op: "peer-close" });
    h.record("peer.disconnect", await closed);
    await h.call("peer.postAfterDisconnect", () =>
      peer.postMessage({ sequence: 3 }),
    );
  } finally {
    chrome.runtime.onMessage.removeListener(listener);
  }
}
