// Implements set-constant (alias set) from uBlock Origin's wiki, Resources Library: "set-constant.js".
function zephiumSetConstant(chain, raw) {
    if (typeof chain !== 'string' || chain === '') {
        return;
    }
    const constant = zephiumConstant(raw);
    if (constant === null) {
        return;
    }
    const isObject = (value) => (typeof value === 'object' && value !== null) || typeof value === 'function';
    const writable = (owner, prop) => {
        const descriptor = Object.getOwnPropertyDescriptor(owner, prop);
        return descriptor === undefined || descriptor.configurable === true;
    };
    // The leaf keeps the constant whatever the page assigns to it later.
    const pin = (owner, prop) => {
        if (!writable(owner, prop)) {
            return;
        }
        Object.defineProperty(owner, prop, {
            configurable: true,
            enumerable: true,
            get() { return constant.value; },
            set() {},
        });
    };
    // An intermediate object may not exist yet, or may be replaced: each object
    // the page puts there is trapped again.
    const trap = (owner, path) => {
        const [prop, ...rest] = path;
        if (rest.length === 0) {
            pin(owner, prop);
            return;
        }
        const current = owner[prop];
        if (writable(owner, prop)) {
            // An accessor already there (another rule's trap, or the page's)
            // keeps working; this trap runs after it.
            const previous = Object.getOwnPropertyDescriptor(owner, prop);
            let value = current;
            const read = previous?.get ? () => previous.get.call(owner) : () => value;
            Object.defineProperty(owner, prop, {
                configurable: true,
                enumerable: true,
                get: read,
                set(next) {
                    if (previous?.set) {
                        previous.set.call(owner, next);
                    } else {
                        value = next;
                    }
                    const stored = read();
                    if (isObject(stored)) {
                        trap(stored, rest);
                    }
                },
            });
        }
        if (isObject(current)) {
            trap(current, rest);
        }
    };
    trap(window, chain.split('.'));
}
