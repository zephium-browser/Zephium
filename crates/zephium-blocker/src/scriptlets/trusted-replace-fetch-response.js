// Implements trusted-replace-fetch-response from uBlock Origin's wiki, Resources Library: "trusted-replace-fetch-response.js".
function zephiumTrustedReplaceFetchResponse(pattern = '', replacement = '', propsToMatch = '', ...options) {
    const replace = zephiumTextReplacer(pattern, replacement, options);
    if (replace === null || typeof fetch !== 'function') {
        return;
    }
    const matches = zephiumRequestMatcher(['propsToMatch', propsToMatch]);
    const details = (resource, init) => {
        const request = typeof Request === 'function' && resource instanceof Request ? resource : null;
        const found = { url: request !== null ? request.url : String(resource) };
        const source = Object.assign({}, request !== null ? { method: request.method, mode: request.mode, credentials: request.credentials } : {}, init);
        for (const [name, value] of Object.entries(source)) {
            if (typeof value === 'string') {
                found[name] = value;
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
                for (const name of ['url', 'redirected', 'type']) {
                    Object.defineProperty(edited, name, { value: response[name] });
                }
                return edited;
            }).catch(() => response));
        },
    });
}
