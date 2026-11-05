// Implements trusted-replace-fetch-response from uBlock Origin's wiki, Resources Library: "trusted-replace-fetch-response.js".
function zephiumTrustedReplaceFetchResponse(pattern = '', replacement = '', propsToMatch = '', ...options) {
    const replace = zephiumTextReplacer(pattern, replacement, options);
    if (replace === null || typeof fetch !== 'function') {
        return;
    }
    const matches = zephiumRequestMatcher(['propsToMatch', propsToMatch]);
    const toString = Object.prototype.toString;
    // fetch() sends the six methods Fetch normalizes in upper case, others as given.
    const method = (text) => {
        const upper = text.toUpperCase();
        return ['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT'].includes(upper) ? upper : text;
    };
    const details = (resource, init) => {
        // By tag, so a request made in a frame's realm is one too.
        const request = Reflect.apply(toString, resource, []) === '[object Request]' ? resource : null;
        const found = { url: request !== null ? request.url : String(resource), method: 'GET' };
        const source = Object.assign({}, request !== null ? { method: request.method, mode: request.mode, credentials: request.credentials } : {}, init);
        for (const [name, value] of Object.entries(source)) {
            if (typeof value === 'string') {
                found[name] = name === 'method' ? method(value) : value;
            }
        }
        return found;
    };
    window.fetch = new Proxy(fetch, {
        apply(target, self, args) {
            const matched = matches(details(args[0], args[1]));
            const pending = Reflect.apply(target, self, args);
            if (!matched) {
                return pending;
            }
            return pending.then((response) => response.clone().text().then((text) => {
                const result = replace(text);
                if (result === null) {
                    return response;
                }
                const edited = new Response(result, {
                    status: response.status,
                    statusText: response.statusText,
                    headers: response.headers,
                });
                // A copy, and a copy of a copy, keeps the metadata a new Response cannot carry.
                const keep = (copy) => {
                    for (const name of ['url', 'redirected', 'type']) {
                        Object.defineProperty(copy, name, { value: response[name] });
                    }
                    const clone = copy.clone;
                    Object.defineProperty(copy, 'clone', {
                        value: function () {
                            return keep(Reflect.apply(clone, this, []));
                        },
                    });
                    return copy;
                };
                return keep(edited);
            }).catch(() => response));
        },
    });
}
