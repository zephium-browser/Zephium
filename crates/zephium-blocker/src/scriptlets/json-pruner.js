// Shared by the json-prune scriptlets: the property paths uBlock Origin's Resources Library documents for "json-prune.js".
function zephiumJsonPruner(removeList, requireList) {
    const paths = (list) => (typeof list === 'string' ? list.split(/\s+/) : [])
        .filter((path) => path !== '')
        .map((path) => path.split('.'));
    const removes = paths(removeList);
    const requires = paths(requireList);
    const isObject = (value) => typeof value === 'object' && value !== null;
    const own = (owner, key) => Object.prototype.hasOwnProperty.call(owner, key);
    // Calls `found(owner, key)` for every property `path` reaches from `node`.
    const reach = (node, path, found) => {
        if (!isObject(node) || path.length === 0) {
            return;
        }
        const [step, ...rest] = path;
        if (step === '[-]' || step === '{-}') {
            if ((step === '[-]') !== Array.isArray(node)) {
                return;
            }
            // Backwards, so removing an array entry keeps the others' places.
            for (const key of Object.keys(node).reverse()) {
                if (rest.length === 0 || has(node[key], rest)) {
                    found(node, key);
                }
            }
            return;
        }
        if (step === '[]' || step === '*') {
            if (step === '[]' && !Array.isArray(node)) {
                return;
            }
            for (const key of Object.keys(node)) {
                if (rest.length === 0) {
                    found(node, key);
                } else {
                    reach(node[key], rest, found);
                }
            }
            return;
        }
        if (!own(node, step)) {
            return;
        }
        if (rest.length === 0) {
            found(node, step);
        } else {
            reach(node[step], rest, found);
        }
    };
    const has = (node, path) => {
        let any = false;
        reach(node, path, () => { any = true; });
        return any;
    };
    // Prunes `value` in place; `changed` says whether anything was removed.
    return (value) => {
        let changed = false;
        if (!isObject(value) || removes.length === 0 || !requires.every((path) => has(value, path))) {
            return { value, changed };
        }
        for (const path of removes) {
            reach(value, path, (owner, key) => {
                if (Array.isArray(owner) && path.includes('[-]')) {
                    owner.splice(Number(key), 1);
                } else {
                    delete owner[key];
                }
                changed = true;
            });
        }
        return { value, changed };
    };
}
