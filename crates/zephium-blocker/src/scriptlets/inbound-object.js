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
    // JSON.stringify only reads its argument, so it gets an edited copy and
    // the page's own object stays as it was; other methods are edited in place.
    const copying = chain === 'JSON.stringify';
    const { parse, stringify } = JSON;
    owner[method] = new Proxy(owner[method], {
        apply(target, self, args) {
            const at = index < 0 ? args.length + index : index;
            if (at >= 0 && at < args.length) {
                try {
                    if (!copying) {
                        path.edit(args[at]);
                    } else if (path.matches(args[at])) {
                        const copy = parse(Reflect.apply(stringify, JSON, [args[at]]));
                        path.edit(copy);
                        args = [...args];
                        args[at] = copy;
                    }
                } catch (e) {
                    // A failed edit leaves the call as the page made it.
                }
            }
            return Reflect.apply(target, self, args);
        },
    });
}
