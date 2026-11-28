// Only the hidden management controller has the native permission bridge.
// Chromium transfers activation on runtime messaging from a privileged context.
(() => {
  const {runtime, webview} = chrome;
  if (!webview) return;
  const origin = runtime.getURL('/');
  const worker = runtime.getManifest().background?.service_worker;
  const workerUrl = worker && runtime.getURL(worker);
  const post = webview.postMessage.bind(webview);
  const open = window.open.bind(window);
  const activation = navigator.userActivation;
  const pending = new Map();
  let next = 0;
  let requestInFlight = false;

  webview.addEventListener('message', ({data}) => {
    if (data?.kind === 'permissions-result') pending.get(data.id)?.(data.result);
  });
  runtime.onMessage.addListener((message, sender, reply) => {
    const op = message?.__zephiumPermissions;
    if (!['request', 'state', 'remove', 'privacy'].includes(op) || sender.id !== runtime.id ||
        !sender.url?.startsWith(origin)) return;
    const deny = () => reply({zephiumPermissions: false});
    const spec = message.spec;
    const valid = spec && (op === 'privacy'
      ? ['get', 'set', 'clear'].includes(spec.op) &&
        ['passwordSavingEnabled', 'autofillAddressEnabled', 'autofillCreditCardEnabled'].includes(spec.setting) &&
        (spec.op !== 'set' || typeof spec.value === 'boolean')
      : ['permissions', 'origins'].every(key => Array.isArray(spec[key]) &&
        spec[key].length <= 16 && spec[key].every(value =>
          typeof value === 'string' && value.length > 0 && value.length <= 256)));
    if (!valid) {
      deny();
      return;
    }
    // Toolbar clicks routed by Zephium are delivered to a worker through our
    // host page. Rust must still attest the exact admitted toolbar gesture.
    const toolbar = op === 'request' && !activation?.isActive && sender.url === workerUrl;
    if (pending.size >= 16 || (op === 'request' && (requestInFlight || (!activation?.isActive && !toolbar)))) {
      deny();
      return;
    }
    const id = ++next;
    const finish = result => {
      if (!pending.delete(id)) return;
      if (op === 'request') requestInFlight = false;
      clearTimeout(timer);
      reply({zephiumPermissions: result});
    };
    // The native ledger expires first. The fallback also covers a closed or
    // unavailable management view and a gesture rejected by WebView2.
    const timer = setTimeout(() => finish(false), op === 'request' ? 27000 : 2000);
    pending.set(id, finish);
    const payload = JSON.stringify({id, spec});
    try {
      if (op === 'request') {
        requestInFlight = true;
        // The native handler denies this fixed local target. It supplies
        // WebView2's IsUserInitiated evidence and consumes the relayed gesture.
        if (toolbar) {
          post(JSON.stringify({kind: 'permissions-toolbar-request', id, spec}));
        } else {
          const target = 'about:blank#zephium-permissions=' + encodeURIComponent(payload);
          if (target.length > 32768) finish(false);
          else open(target, '_blank');
        }
      } else if (op === 'privacy') {
        post(JSON.stringify({kind: 'permissions-privacy', id,
          op: spec.op, setting: spec.setting, value: spec.value}));
      } else {
        post(JSON.stringify({kind: 'permissions-' + op, id, spec}));
      }
    } catch {
      finish(false);
    }
    return true;
  });
})();
