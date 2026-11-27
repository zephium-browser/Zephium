// Implements json-prune-xhr-response from uBlock Origin's wiki, Resources Library: "json-prune-xhr-response.js".
function zephiumJsonPruneXhrResponse(removeList = '', requireList = '', ...options) {
    if (removeList === '' || typeof XMLHttpRequest !== 'function') {
        return;
    }
    const prune = zephiumJsonPruner(removeList, requireList);
    const matches = zephiumRequestMatcher(options);
    const { parse, stringify } = JSON;
    const proto = XMLHttpRequest.prototype;
    const requests = new WeakMap();
    proto.open = new Proxy(proto.open, {
        apply(target, self, args) {
            requests.set(self, { matched: matches({ method: String(args[0]), url: String(args[1]) }) });
            return Reflect.apply(target, self, args);
        },
    });
    const text = Object.getOwnPropertyDescriptor(proto, 'responseText');
    const response = Object.getOwnPropertyDescriptor(proto, 'response');
    if (!text?.get || !response?.get) {
        return;
    }
    // The pruned body, worked out once per request when it is first read.
    const edited = (xhr) => {
        const entry = requests.get(xhr);
        if (entry === undefined || !entry.matched || xhr.readyState !== 4) {
            return null;
        }
        if (entry.result === undefined) {
            entry.result = null;
            try {
                if (xhr.responseType === 'json') {
                    const value = Reflect.apply(response.get, xhr, []);
                    entry.result = { value: prune(value).value };
                } else if (xhr.responseType === '' || xhr.responseType === 'text') {
                    const result = prune(parse(Reflect.apply(text.get, xhr, [])));
                    entry.result = result.changed ? { value: stringify(result.value) } : null;
                }
            } catch (e) {
                entry.result = null;
            }
        }
        return entry.result;
    };
    Object.defineProperty(proto, 'responseText', {
        ...text,
        get: new Proxy(text.get, {
            apply(target, self, args) {
                const result = edited(self);
                return result !== null && typeof result.value === 'string' ? result.value : Reflect.apply(target, self, args);
            },
        }),
    });
    Object.defineProperty(proto, 'response', {
        ...response,
        get: new Proxy(response.get, {
            apply(target, self, args) {
                const result = edited(self);
                return result !== null ? result.value : Reflect.apply(target, self, args);
            },
        }),
    });
}
