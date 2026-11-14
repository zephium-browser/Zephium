export function createHarness(config) {
  const observations = [];
  const clean = (value) =>
    value === undefined
      ? { $undefined: true }
      : JSON.parse(JSON.stringify(value));
  const h = {
    origins: config.origins,
    observations,
    record(label, value) {
      observations.push({ label, value: clean(value) });
    },
    assert(value, message) {
      if (!value) throw new Error(message);
    },
    sleep(ms) {
      return new Promise((resolve) => setTimeout(resolve, ms));
    },
    async call(label, invoke) {
      h.record(`${label}:call`, { timing: "synchronous" });
      try {
        const pending = invoke();
        const timing = pending?.then ? "promise" : "synchronous";
        const value = await pending;
        h.record(`${label}:result`, {
          timing,
          value: clean(value),
          lastError: chrome.runtime.lastError?.message ?? null,
        });
        return value;
      } catch (error) {
        h.record(`${label}:error`, {
          message: error.message,
          lastError: chrome.runtime.lastError?.message ?? null,
        });
        return undefined;
      }
    },
    callback(label, invoke) {
      h.record(`${label}:call`, { timing: "synchronous" });
      return new Promise((resolve, reject) => {
        let returned = false;
        try {
          invoke((...args) => {
            h.record(`${label}:callback`, {
              args: args.map(clean),
              lastError: chrome.runtime.lastError?.message ?? null,
              timing: returned ? "asynchronous" : "synchronous",
            });
            queueMicrotask(() => {
              h.record(`${label}:after`, {
                lastError: chrome.runtime.lastError?.message ?? null,
              });
              setTimeout(() => {
                h.record(`${label}:afterTask`, {
                  lastError: chrome.runtime.lastError?.message ?? null,
                });
                resolve(args);
              }, 0);
            });
          });
          returned = true;
        } catch (error) {
          reject(error);
        }
      });
    },
    event(event, predicate = () => true, timeoutMs = 5000) {
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          event.removeListener(listener);
          reject(new Error("Timed out waiting for event"));
        }, timeoutMs);
        function listener(...args) {
          if (!predicate(...args)) return;
          clearTimeout(timer);
          event.removeListener(listener);
          resolve(args);
        }
        event.addListener(listener);
      });
    },
    async openTab(url) {
      const tab = await chrome.tabs.create({ url, active: false });
      if (tab.status !== "complete") {
        await new Promise((resolve, reject) => {
          const timer = setTimeout(() => {
            chrome.tabs.onUpdated.removeListener(listener);
            reject(new Error(`Tab load timed out: ${url}`));
          }, 10000);
          function listener(id, change) {
            if (id === tab.id && change.status === "complete") {
              clearTimeout(timer);
              chrome.tabs.onUpdated.removeListener(listener);
              resolve();
            }
          }
          chrome.tabs.onUpdated.addListener(listener);
          chrome.tabs.get(tab.id).then((current) => {
            if (current.status === "complete")
              listener(tab.id, { status: "complete" });
          }, reject);
        });
      }
      return chrome.tabs.get(tab.id);
    },
    async host(operation, args = {}) {
      const response = await fetch(`${config.collector}/control`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ operation, args }),
      });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error);
      return result;
    },
  };
  return h;
}
