// Reports native action changes. No tab, permission or action dispatch emulation.
(() => {
  // Extensions such as Proton Pass replace the worker's `chrome` global later;
  // keep the native objects seen at load time.
  const action = chrome.action;
  const runtime = chrome.runtime;
  if (!action || !runtime) return;
  const icons = new Map();
  const notify = () => {
    try { runtime.sendMessage({__zephiumActionChanged: true}).catch(() => {}); } catch { /* Never reach the extension's own call. */ }
  };
  for (const method of ['setTitle', 'setBadgeText', 'setIcon', 'setPopup', 'enable', 'disable']) {
    const original = action[method];
    action[method] = function (...args) {
      const result = Reflect.apply(original, this, args);
      const report = () => {
        if (method === 'setIcon') {
          const details = args[0] || {};
          const images = details.imageData;
          const image = images?.data ? images : images?.[32] || images?.[16];
          const path = typeof details.path === 'string' ? details.path : details.path?.[32] || details.path?.[16];
          let icon;
          if (image && image.width <= 128 && image.height <= 128 && image.data.length <= 65536)
            icon = {width: image.width, height: image.height, data: Array.from(image.data)};
          else if (typeof path === 'string' && path.length < 2048) icon = {path};
          if (icon) {
            const key = Number.isInteger(details.tabId) ? details.tabId : -1;
            if (icons.size >= 128 && !icons.has(key)) icons.delete(icons.keys().next().value);
            icons.set(key, icon);
          }
        }
        notify();
      };
      if (result?.then) result.then(report, () => {});
      else report();
      return result;
    };
  }
  runtime.onMessage.addListener((message, sender, reply) => {
    if (sender.id === runtime.id && message?.__zephiumActionSnapshot === true) {
      reply({icon: icons.get(message.tabId) || icons.get(-1) || null,
        perTabIcons: icons.size > (icons.has(-1) ? 1 : 0)});
    }
  });
})();
