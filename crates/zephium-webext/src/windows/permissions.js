// Packaged before extension code in the MV3 worker and privileged documents.
(() => {
  const {permissions, runtime} = chrome;
  if (!permissions || !runtime) return;
  const manifest = runtime.getManifest();
  const optional = {
    permissions: new Set((manifest.optional_permissions || []).filter(value => !value.includes('://') && value !== '<all_urls>')),
    origins: new Set([...(manifest.optional_host_permissions || []),
      ...(manifest.optional_permissions || []).filter(value => value.includes('://') || value === '<all_urls>')])
  };
  const documentContext = typeof document !== 'undefined';
  const active = documentContext ? navigator.userActivation : null;
  const send = runtime.sendMessage.bind(runtime);
  const contains = permissions.contains.bind(permissions);
  const getAll = permissions.getAll.bind(permissions);
  const remove = permissions.remove.bind(permissions);
  const empty = () => ({permissions: [], origins: []});
  const fields = ['permissions', 'origins'];
  const valid = values => Array.isArray(values) && values.length <= 16 &&
    values.every(value => typeof value === 'string' && value.length > 0 && value.length <= 256);
  function normalize(value) {
    if (!value || typeof value !== 'object') return null;
    const spec = empty();
    for (const key of fields) {
      const values = Object.hasOwn(value, key) ? value[key] : [];
      if (!valid(values)) return null;
      spec[key] = [...new Set(values)];
    }
    return spec;
  }
  function bridge(op, spec) {
    try {
      return Promise.resolve(send({__zephiumPermissions: op, spec}))
        .then(result => result?.zephiumPermissions, () => undefined);
    } catch {
      return Promise.resolve(undefined);
    }
  }
  function callbackOrPromise(promise, callback) {
    if (typeof callback !== 'function') return promise;
    promise.then(callback, error => {
      const previous = Object.getOwnPropertyDescriptor(runtime, 'lastError');
      try {
        Object.defineProperty(runtime, 'lastError', {configurable: true, value: {message: String(error.message ?? error)}});
        callback();
      } finally {
        if (previous) Object.defineProperty(runtime, 'lastError', previous);
        else delete runtime.lastError;
      }
    });
  }
  let pending = false;
  permissions.request = function (value, callback) {
    const spec = normalize(value);
    let answer = Promise.resolve(false);
    if (spec && !pending && (!documentContext || active?.isActive) &&
        fields.every(key => spec[key].every(item => optional[key].has(item)))) {
      // Start messaging synchronously so Chromium transfers the worker or
      // document's activation to the management page.
      pending = true;
      answer = bridge('request', spec).then(result => result === true).finally(() => { pending = false; });
    }
    return callbackOrPromise(answer, callback);
  };
  permissions.contains = function (value, callback) {
    const spec = normalize(value);
    const answer = !spec ? Promise.resolve(false) : contains(spec).then(async native => {
      if (native) return true;
      const local = await bridge('state', empty());
      if (!local) return false;
      const remaining = Object.fromEntries(fields.map(key => [
        key, spec[key].filter(item => !local[key]?.includes(item))
      ]));
      return fields.every(key => remaining[key].length === 0) || contains(remaining);
    });
    return callbackOrPromise(answer, callback);
  };
  permissions.getAll = function (callback) {
    const answer = Promise.all([getAll(), bridge('state', empty())]).then(([native, local]) =>
      Object.fromEntries(fields.map(key => [key, [...new Set([...(native[key] || []), ...(local?.[key] || [])])]])));
    return callbackOrPromise(answer, callback);
  };
  permissions.remove = function (value, callback) {
    const spec = normalize(value);
    const answer = !spec ? Promise.resolve(false) : remove(spec).then(async native =>
      native && await bridge('remove', spec) === true);
    return callbackOrPromise(answer, callback);
  };

  // WebView2 cannot dynamically expose Chromium's optional privacy namespace.
  // Zephium implements the autofill settings its Windows backend can control.
  if (optional.permissions.has('privacy') && !chrome.privacy) {
    const services = {};
    for (const setting of ['passwordSavingEnabled', 'autofillAddressEnabled', 'autofillCreditCardEnabled']) {
      const call = (op, details, callback) => {
        const regular = !details?.incognito && (details?.scope === undefined || details.scope === 'regular');
        const answer = regular ? bridge('privacy', {op, setting, value: details?.value}) : Promise.resolve(false);
        return callbackOrPromise(answer.then(result => {
          if (result === undefined || result === false) throw new Error('Privacy setting unavailable');
          return op === 'get' ? result : undefined;
        }), callback);
      };
      services[setting] = {
        get: (details, callback) => call('get', details, callback),
        set: (details, callback) => call('set', details, callback),
        clear: (details, callback) => call('clear', details, callback)
      };
    }
    Object.defineProperty(chrome, 'privacy', {value: {services}, configurable: true});
  }
})();
