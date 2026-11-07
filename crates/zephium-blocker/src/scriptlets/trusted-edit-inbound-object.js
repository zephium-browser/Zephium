// Implements trusted-edit-inbound-object from uBlock Origin's wiki, Resources Library: "edit-inbound-object" and its trusted version, which may assign values: the method's property chain, the 0-based position of the argument to edit (negative counts from the end), and the query.
function zephiumTrustedEditInboundObject(chain, position, query) {
    const edit = zephiumObjectEditor(query);
    const index = Number(position);
    const names = typeof chain === 'string' ? chain.split('.') : [];
    if (edit === undefined || !Number.isInteger(index) || names.length === 0 || names.some((name) => !/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name))) {
        return;
    }
    let owner = globalThis;
    for (const name of names.slice(0, -1)) {
        owner = owner[name];
        if (owner === null || (typeof owner !== 'object' && typeof owner !== 'function')) {
            return;
        }
    }
    const method = names[names.length - 1];
    if (typeof owner[method] !== 'function') {
        return;
    }
    owner[method] = new Proxy(owner[method], {
        apply(target, thisArg, args) {
            const at = index < 0 ? args.length + index : index;
            if (at >= 0 && at < args.length) {
                edit(args[at]);
            }
            return Reflect.apply(target, thisArg, args);
        },
    });
}
