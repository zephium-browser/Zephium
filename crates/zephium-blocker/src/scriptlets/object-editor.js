// Shared by the edit scriptlets: a query from uBlock Origin's wiki, "uBO flavored JSONPath", as an edit of one object, or undefined when the query is not one this library reads. Two forms are read: the wiki's v2 grammar, without call() so a list never names a function to run, and the older form today's YouTube rules are written in, read from those rules alone: dotted names and filters on the current node, ended by an assignment. The compiler admits only queries scriptlets.rs's object_query module accepts; the two readers must agree.
function zephiumObjectEditor(query) {
    const text = typeof query === 'string' ? query : '';
    const MAX_NODES = 10000;
    const UNSAFE = new Set(['__proto__', 'constructor', 'prototype']);
    let at = 0;
    const fail = () => { throw new SyntaxError('query'); };
    const peek = (token) => text.startsWith(token, at);
    const take = (token) => { if (peek(token)) { at += token.length; return true; } return false; };
    const isObject = (value) => value !== null && typeof value === 'object';
    const isPlainObject = (value) => isObject(value) && !Array.isArray(value);
    const own = (value, key) => isObject(value) && Object.prototype.hasOwnProperty.call(value, key);
    const name = () => {
        const match = /^[A-Za-z_][A-Za-z0-9_]*/.exec(text.slice(at));
        if (match === null || UNSAFE.has(match[0])) { fail(); }
        at += match[0].length;
        return match[0];
    };
    const jsonString = () => {
        if (!peek('"')) { fail(); }
        let end = at + 1;
        while (end < text.length && text[end] !== '"') { end += text[end] === '\\' ? 2 : 1; }
        if (end >= text.length) { fail(); }
        const value = JSON.parse(text.slice(at, end + 1));
        at = end + 1;
        return value;
    };
    const quotedName = () => {
        if (peek('"')) {
            const value = jsonString();
            if (UNSAFE.has(value)) { fail(); }
            return value;
        }
        if (!take("'")) { fail(); }
        const end = text.indexOf("'", at);
        if (end < 0) { fail(); }
        const value = text.slice(at, end);
        if (value.includes('\\') || UNSAFE.has(value)) { fail(); }
        at = end + 1;
        return value;
    };
    const regex = () => {
        if (!take('/')) { fail(); }
        let end = at;
        let inClass = false;
        while (end < text.length && (inClass || text[end] !== '/')) {
            if (text[end] === '\\') { end += 1; } else if (text[end] === '[') { inClass = true; } else if (text[end] === ']') { inClass = false; }
            end += 1;
        }
        if (end >= text.length || end === at) { fail(); }
        const body = text.slice(at, end);
        at = end + 1;
        const flags = /^[imsu]*/.exec(text.slice(at))[0];
        at += flags.length;
        return new RegExp(body, flags);
    };
    const scalar = () => {
        if (peek('"')) { return jsonString(); }
        const match = /^(?:-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?|true|false|null)/.exec(text.slice(at));
        if (match === null) { fail(); }
        at += match[0].length;
        return JSON.parse(match[0]);
    };
    const safeJson = (source) => {
        const value = JSON.parse(source);
        const check = (node) => {
            if (!isObject(node)) { return; }
            for (const key of Object.keys(node)) {
                if (UNSAFE.has(key)) { fail(); }
                check(node[key]);
            }
        };
        check(value);
        return value;
    };
    // `${now}` is not on the wiki; read from the rules, it stands for the time of the edit, Date.now() as a decimal string.
    const assignment = () => {
        if (at === text.length) { return undefined; }
        if (take('+=')) {
            const source = text.slice(at);
            if (!isPlainObject(safeJson(source))) { fail(); }
            at = text.length;
            return { merge: source };
        }
        if (!take('=')) { fail(); }
        if (take('repl(')) {
            if (!text.endsWith(')')) { fail(); }
            const spec = safeJson(text.slice(at, -1));
            at = text.length;
            const keys = isPlainObject(spec) ? Object.keys(spec).sort().join(',') : '';
            if (keys === 'regex,replacement' && typeof spec.regex === 'string' && spec.regex !== '' && typeof spec.replacement === 'string') {
                // A JSON "\b" is a backspace, which no pattern means; read from the rules, it is a word boundary.
                const source = spec.regex.replace(/\u0008/g, '\\b');
                const literal = /^\/(.+)\/([imsu]*)$/.exec(source);
                return { replace: literal === null ? new RegExp(source) : new RegExp(literal[1], literal[2]), with: spec.replacement };
            }
            if (keys === 'pattern,replacement' && typeof spec.pattern === 'string' && spec.pattern !== '' && typeof spec.replacement === 'string') {
                return { replace: spec.pattern, with: spec.replacement };
            }
            fail();
        }
        if (peek('call(')) { fail(); }
        const source = text.slice(at);
        const value = safeJson(source);
        at = text.length;
        if (value === '${now}') { return { now: true }; }
        if (source.includes('${')) { fail(); }
        return { set: source };
    };
    const resolve = (value, path) => {
        let current = value;
        for (const key of path) {
            if (!own(current, key)) { return undefined; }
            current = current[key];
        }
        return current;
    };
    const compare = (operator, left, right) => {
        switch (operator) {
            case '==': return left === right;
            case '!=': return left !== right;
            case '*=': return typeof left === 'string' && typeof right === 'string' && left.includes(right);
            case '^=': return typeof left === 'string' && typeof right === 'string' && left.startsWith(right);
            case '$=': return typeof left === 'string' && typeof right === 'string' && left.endsWith(right);
            default: {
                const comparable = (typeof left === 'number' && typeof right === 'number') || (typeof left === 'string' && typeof right === 'string');
                if (!comparable) { return false; }
                return operator === '<' ? left < right : operator === '<=' ? left <= right : operator === '>' ? left > right : left >= right;
            }
        }
    };
    const predicate = (path, test) => (value) => {
        const found = resolve(value, path);
        if (test === undefined) { return found !== undefined; }
        if (test.regex !== undefined) { return typeof found === 'string' && test.regex.test(found); }
        return found !== undefined && compare(test.operator, found, test.value);
    };
    const parseTest = (operators, literal) => {
        if (peek('=/')) { at += 1; return { regex: regex() }; }
        for (const operator of operators) {
            if (take(operator)) { return { operator, value: literal() }; }
        }
        return undefined;
    };

    const legacy = () => {
        const steps = [];
        for (;;) {
            if (take('[?')) {
                if (!take('.')) { fail(); }
                const path = [name()];
                while (take('.')) { path.push(name()); }
                const test = parseTest(['*=', '=='], jsonString);
                if (!take(']')) { fail(); }
                steps.push({ filter: predicate(path, test) });
            } else if (peek('.') && !peek('..')) {
                at += 1;
                steps.push({ name: name() });
            } else {
                break;
            }
        }
        const action = assignment();
        if (steps.length === 0 || action === undefined || at !== text.length) { fail(); }
        return { queries: [{ steps, quantity: [1, Infinity] }], action };
    };

    const V2_OPERATORS = ['==', '!=', '<=', '>=', '<', '>', '^=', '$=', '*='];
    const selector = () => {
        if (take('*')) { return { all: true }; }
        if (take('?')) {
            if (!take('@')) { fail(); }
            const path = [];
            for (;;) {
                if (peek('.') && !peek('..')) { at += 1; path.push(name()); } else if (take('[')) { path.push(quotedName()); if (!take(']')) { fail(); } } else { break; }
            }
            return { filter: predicate(path, parseTest(V2_OPERATORS, scalar)) };
        }
        if (peek('/')) { return { keys: regex() }; }
        const index = /^-?(?:0|[1-9][0-9]*)/.exec(text.slice(at));
        if (index !== null) { at += index[0].length; return { index: Number(index[0]) }; }
        const names = [quotedName()];
        while (take(',')) { names.push(quotedName()); }
        return { names };
    };
    const v2Query = () => {
        if (!take('$')) { fail(); }
        const steps = [];
        for (;;) {
            let deep = false;
            if (take('..')) { deep = true; } else if (!take('.')) {
                if (!peek('[')) { break; }
            }
            let select;
            if (take('[')) { select = selector(); if (!take(']')) { fail(); } } else if (take('*')) { select = { all: true }; } else if (peek('/')) { select = { keys: regex() }; } else { select = { names: [name()] }; }
            steps.push({ deep, select });
        }
        let quantity = [1, Infinity];
        let bounded = false;
        {
            const bounds = /^\{([0-9]*)(,?)([0-9]*)\}/.exec(text.slice(at));
            if (bounds !== null && (bounds[1] !== '' || bounds[3] !== '')) {
                at += bounds[0].length;
                const low = bounds[1] === '' ? 0 : Number(bounds[1]);
                const high = bounds[2] === '' ? low : bounds[3] === '' ? Infinity : Number(bounds[3]);
                quantity = [low, high];
                bounded = true;
            }
        }
        return { steps, quantity, bounded };
    };
    const v2 = () => {
        take('v2:');
        const queries = [v2Query()];
        while (take(';')) { queries.push(v2Query()); }
        if (queries[queries.length - 1].bounded) { fail(); }
        const action = assignment() ?? { remove: true };
        if (at !== text.length) { fail(); }
        return { queries, action };
    };

    let plan;
    try {
        if (text === '' || text.length > 4096) { return undefined; }
        plan = text.startsWith('$') || text.startsWith('v2:') ? v2() : legacy();
    } catch (e) {
        return undefined;
    }

    const children = (node, select, push) => {
        const value = node.value;
        if (!isObject(value)) { return; }
        const entries = Array.isArray(value) ? value.map((child, index) => [index, child]) : Object.entries(value);
        if (select.index !== undefined) {
            if (!Array.isArray(value)) { return; }
            const index = select.index < 0 ? value.length + select.index : select.index;
            if (index >= 0 && index < value.length) { push({ value: value[index], parent: value, key: index }); }
            return;
        }
        for (const [key, child] of entries) {
            const keep = select.all === true
                || (select.names !== undefined && !Array.isArray(value) && select.names.includes(key))
                || (select.keys !== undefined && typeof key === 'string' && select.keys.test(key))
                || (select.filter !== undefined && select.filter(child));
            if (keep) { push({ value: child, parent: value, key }); }
        }
    };
    const run = (steps, root) => {
        let nodes = [{ value: root, parent: undefined, key: undefined }];
        let visited = 0;
        const count = () => { visited += 1; if (visited > MAX_NODES) { throw new RangeError('size'); } };
        for (const step of steps) {
            const next = [];
            const push = (node) => { count(); next.push(node); };
            if (step.filter !== undefined) {
                nodes = nodes.filter((node) => step.filter(node.value));
                continue;
            }
            if (step.name !== undefined) {
                for (const node of nodes) {
                    if (own(node.value, step.name)) { push({ value: node.value[step.name], parent: node.value, key: step.name }); }
                }
            } else if (step.deep) {
                const stack = [...nodes];
                while (stack.length > 0) {
                    const node = stack.pop();
                    count();
                    children(node, step.select, push);
                    children(node, { all: true }, (child) => stack.push(child));
                }
            } else {
                for (const node of nodes) { children(node, step.select, push); }
            }
            nodes = next;
        }
        return nodes;
    };
    const apply = (action, nodes) => {
        if (action.remove) {
            const indexes = new Map();
            for (const node of nodes) {
                if (Array.isArray(node.parent)) {
                    if (!indexes.has(node.parent)) { indexes.set(node.parent, []); }
                    indexes.get(node.parent).push(node.key);
                } else if (node.parent !== undefined) {
                    delete node.parent[node.key];
                }
            }
            for (const [array, keys] of indexes) {
                for (const index of [...new Set(keys)].sort((a, b) => b - a)) { array.splice(index, 1); }
            }
            return;
        }
        for (const node of nodes) {
            if (action.merge !== undefined) {
                if (isPlainObject(node.value)) { Object.assign(node.value, JSON.parse(action.merge)); }
            } else if (node.parent === undefined) {
                continue;
            } else if (action.now) {
                node.parent[node.key] = String(Date.now());
            } else if (action.set !== undefined) {
                node.parent[node.key] = JSON.parse(action.set);
            } else if (typeof node.value === 'string') {
                node.parent[node.key] = node.value.replace(action.replace, action.with);
            }
        }
    };
    return (root) => {
        if (!isObject(root)) { return false; }
        try {
            let selected = [];
            for (const query of plan.queries) {
                selected = run(query.steps, root);
                if (selected.length < query.quantity[0] || selected.length > query.quantity[1]) { return false; }
            }
            apply(plan.action, selected);
            return true;
        } catch (e) {
            return false;
        }
    };
}
