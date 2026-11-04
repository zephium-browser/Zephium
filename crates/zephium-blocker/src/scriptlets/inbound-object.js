// Shared by the inbound-object scriptlets: the method chain, argument position and query uBlock Origin's wiki documents for "edit-inbound-object".
function zephiumInboundObject(chain, position, query, trusted) {
    if (typeof chain !== 'string' || chain === '') {
        return;
    }
    const index = parseInt(position, 10);
    if (!Number.isInteger(index)) {
        return;
    }
    const path = zephiumJsonPath(query, trusted);
    const names = chain.split('.');
    const method = names.pop();
    let owner = window;
    for (const name of names) {
        owner = owner?.[name];
    }
    if (typeof owner?.[method] !== 'function') {
        return;
    }
    owner[method] = new Proxy(owner[method], {
        apply(target, self, args) {
            const at = index < 0 ? args.length + index : index;
            if (at >= 0 && at < args.length) {
                try {
                    path.edit(args[at]);
                } catch (e) {
                    // A failed edit leaves the call as the page made it.
                }
            }
            return Reflect.apply(target, self, args);
        },
    });
}
