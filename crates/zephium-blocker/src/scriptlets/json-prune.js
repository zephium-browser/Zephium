// Implements json-prune from uBlock Origin's wiki, Resources Library: "json-prune.js".
function zephiumJsonPrune(removeList = '', requireList = '', stackNeedle = '') {
    if (removeList === '') {
        return;
    }
    const prune = zephiumJsonPruner(removeList, requireList);
    const stack = stackNeedle === '' ? null : zephiumPattern(stackNeedle);
    const fromHere = () => stack === null || stack(String(new Error().stack));
    JSON.parse = new Proxy(JSON.parse, {
        apply(target, self, args) {
            const value = Reflect.apply(target, self, args);
            return fromHere() ? prune(value).value : value;
        },
    });
    if (typeof Response === 'function') {
        Response.prototype.json = new Proxy(Response.prototype.json, {
            apply(target, self, args) {
                const matched = fromHere();
                const pending = Reflect.apply(target, self, args);
                return matched ? pending.then((value) => prune(value).value) : pending;
            },
        });
    }
}
