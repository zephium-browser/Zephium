// Implements trusted-replace-xhr-response from uBlock Origin's wiki, Resources Library: "trusted-replace-xhr-response.js".
function zephiumTrustedReplaceXhrResponse(pattern = '', replacement = '', propsToMatch = '', ...options) {
    const replace = zephiumTextReplacer(pattern, replacement, options);
    if (replace === null || typeof XMLHttpRequest !== 'function') {
        return;
    }
    const matches = zephiumRequestMatcher(['propsToMatch', propsToMatch]);
    const { parse, stringify } = JSON;
    // open() sends the six methods Fetch normalizes in upper case, others as given.
    const method = (value) => {
        const text = String(value);
        const upper = text.toUpperCase();
        return ['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT'].includes(upper) ? upper : text;
    };
    const proto = XMLHttpRequest.prototype;
    const requests = new WeakMap();
    proto.open = new Proxy(proto.open, {
        apply(target, self, args) {
            requests.set(self, { matched: matches({ method: method(args[0]), url: String(args[1]) }) });
            return Reflect.apply(target, self, args);
        },
    });
    const text = Object.getOwnPropertyDescriptor(proto, 'responseText');
    const response = Object.getOwnPropertyDescriptor(proto, 'response');
    if (!text?.get || !response?.get) {
        return;
    }
    // The edited body, worked out once per request when it is first read; a body that is not text is left alone.
    const edited = (xhr) => {
        const entry = requests.get(xhr);
        if (entry === undefined || !entry.matched || xhr.readyState !== 4) {
            return null;
        }
        if (entry.result === undefined) {
            entry.result = null;
            try {
                if (xhr.responseType === '' || xhr.responseType === 'text') {
                    const result = replace(Reflect.apply(text.get, xhr, []));
                    entry.result = result === null ? null : { value: result };
                } else if (xhr.responseType === 'json') {
                    const result = replace(stringify(Reflect.apply(response.get, xhr, [])));
                    entry.result = result === null ? null : { value: parse(result) };
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
                // Other types throw natively, and still do.
                const textual = self.responseType === '' || self.responseType === 'text';
                const result = textual ? edited(self) : null;
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
