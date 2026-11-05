// Implements trusted-prevent-dom-bypass from uBlock Origin's wiki, Resources Library: "trusted-prevent-dom-bypass.js".
function zephiumTrustedPreventDomBypass(methodPath = '', targetProp = '') {
    const path = typeof methodPath === 'string' ? methodPath.split('.') : [];
    const name = path.pop();
    let owner = window;
    for (const part of path) {
        owner = owner?.[part];
    }
    if (path.length === 0 || typeof owner?.[name] !== 'function') {
        return;
    }
    const parent = window;
    // Points the embedded context's `targetProp` at the parent's patched one, or the whole context at the parent when none is named.
    const delegate = (frame) => {
        try {
            const view = frame.contentWindow;
            if (!view || view === parent) {
                return;
            }
            if (targetProp === '') {
                Object.defineProperty(frame, 'contentWindow', { value: parent, configurable: true });
                Object.defineProperty(frame, 'contentDocument', { value: parent.document, configurable: true });
                return;
            }
            const steps = targetProp.split('.');
            const last = steps.pop();
            let source = parent;
            let target = view;
            for (const step of steps) {
                source = source?.[step];
                target = target?.[step];
            }
            if (source == null || target == null || !(last in source)) {
                return;
            }
            const value = source[last];
            // A call made on the embedded window runs on the parent's.
            const delegated = typeof value === 'function'
                ? new Proxy(value, { apply: (fn, self, args) => Reflect.apply(fn, self === view ? parent : self, args) })
                : value;
            Object.defineProperty(target, last, { value: delegated, configurable: true, writable: true });
        } catch (e) {
        }
    };
    // Captured, so a named form control cannot shadow it on the inserted node.
    const select = typeof Element === 'function' ? Element.prototype.querySelectorAll : null;
    const inside = (node) => {
        try {
            return select === null ? [] : [...Reflect.apply(select, node, ['iframe,frame'])];
        } catch (e) {
            return [];
        }
    };
    // A frame's own script can run before the insertion returns, such as from
    // a custom element's connectedCallback, and keep the original function.
    owner[name] = new Proxy(owner[name], {
        apply(target, self, args) {
            // The native call empties a fragment, so its frames are found first.
            const frames = [];
            for (const node of args) {
                if (typeof DocumentFragment === 'function' && node instanceof DocumentFragment) {
                    frames.push(...inside(node));
                } else if (typeof HTMLElement === 'function' && node instanceof HTMLElement) {
                    frames.push(node, ...inside(node));
                }
            }
            const result = Reflect.apply(target, self, args);
            for (const frame of frames) {
                if ('contentWindow' in frame) {
                    delegate(frame);
                }
            }
            return result;
        },
    });
}
