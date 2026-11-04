// Shared by the edit scriptlets: the query and assignment syntax uBlock Origin's wiki documents in "uBO flavored JSONPath".
function zephiumJsonPath(query, trusted) {
    const never = { edit: () => false };
    if (typeof query !== 'string' || query === '') {
        return never;
    }
    let text = query;
    // A query without `v2:` or `$` is the older form uBO's lists still use,
    // as in `[?.a][?.b.c*="x"].d+={"e":1}`. The wiki documents only v2; from
    // those rules, its filters test the node they follow, not its children.
    let legacy = true;
    if (text.startsWith('v2:')) {
        text = text.slice(3);
        legacy = false;
    }
    if (text.startsWith('$')) {
        text = text.slice(1);
        legacy = false;
    }
    let at = 0;
    const fail = () => {
        throw new SyntaxError(query);
    };
    const isObject = (value) => typeof value === 'object' && value !== null;
    const quoted = () => {
        const quote = text[at];
        let out = '';
        at += 1;
        while (at < text.length && text[at] !== quote) {
            if (text[at] === '\\' && at + 1 < text.length) {
                at += 1;
            }
            out += text[at];
            at += 1;
        }
        if (text[at] !== quote) {
            fail();
        }
        at += 1;
        return out;
    };
    const regex = () => {
        const end = /^\/((?:\\.|[^\\/])+)\/([imsu]*)/.exec(text.slice(at));
        if (end === null) {
            fail();
        }
        at += end[0].length;
        return new RegExp(end[1], end[2]);
    };
    const name = () => {
        const word = /^[A-Za-z_$][\w$-]*/.exec(text.slice(at));
        if (word === null) {
            fail();
        }
        at += word[0].length;
        return word[0];
    };
    // A JSON literal, read by trying the longest prefix that parses.
    const literal = () => {
        const rest = text.slice(at);
        for (let end = rest.length; end > 0; end -= 1) {
            try {
                const value = JSON.parse(rest.slice(0, end));
                at += end;
                return value;
            } catch (e) {
                // A shorter prefix may parse.
            }
        }
        fail();
    };
    // A relative path inside a filter: `.a.b`, `@.a`, `['a']`.
    const relative = () => {
        if (text[at] === '@') {
            at += 1;
        }
        const steps = [];
        for (;;) {
            if (text[at] === '.' && text[at + 1] !== '.') {
                at += 1;
                steps.push({ kind: 'name', key: name() });
            } else if (text[at] === '[' && (text[at + 1] === '"' || text[at + 1] === "'")) {
                at += 1;
                steps.push({ kind: 'name', key: quoted() });
                if (text[at] !== ']') {
                    fail();
                }
                at += 1;
            } else {
                return steps;
            }
        }
    };
    const filter = () => {
        const negated = text[at] === '!';
        if (negated) {
            at += 1;
        }
        const path = relative();
        const op = /^(==|!=|<=|>=|<|>|\^=|\$=|\*=|=(?=\/))/.exec(text.slice(at));
        if (op === null) {
            return { path, negated, test: () => true };
        }
        at += op[0].length;
        if (op[0] === '=') {
            const pattern = regex();
            return { path, negated, test: (value) => typeof value === 'string' && pattern.test(value) };
        }
        const operand = literal();
        const tests = {
            '==': (value) => value === operand,
            '!=': (value) => value !== operand,
            '<': (value) => value < operand,
            '<=': (value) => value <= operand,
            '>': (value) => value > operand,
            '>=': (value) => value >= operand,
            '^=': (value) => typeof value === 'string' && value.startsWith(operand),
            '$=': (value) => typeof value === 'string' && value.endsWith(operand),
            '*=': (value) => typeof value === 'string' && value.includes(operand),
        };
        return { path, negated, test: tests[op[0]] };
    };
    const segments = [];
    const assignment = () => {
        if (text.startsWith('+=', at)) {
            at += 2;
            const value = literal();
            if (!isObject(value) || Array.isArray(value)) {
                fail();
            }
            return { kind: 'merge', value };
        }
        at += 1;
        const repl = /^repl\((.*)\)$/.exec(text.slice(at));
        if (repl !== null) {
            at = text.length;
            const spec = JSON.parse(repl[1]);
            let find = null;
            if (typeof spec.pattern === 'string') {
                find = spec.pattern;
            } else if (typeof spec.regex === 'string') {
                const slashed = /^\/(.+)\/([dgimsuvy]*)$/.exec(spec.regex);
                find = slashed === null ? new RegExp(spec.regex) : new RegExp(slashed[1], slashed[2]);
            }
            if (find === null || typeof spec.replacement !== 'string') {
                fail();
            }
            return { kind: 'replace', find, replacement: spec.replacement };
        }
        return { kind: 'set', value: literal() };
    };
    let change = null;
    try {
        while (at < text.length) {
            if (text.startsWith('..', at)) {
                at += 2;
                if (text[at] === '[') {
                    segments.push({ kind: 'descend' });
                } else {
                    segments.push({ kind: 'descend' }, text[at] === '*' ? (at += 1, { kind: 'all' }) : { kind: 'name', key: name() });
                }
            } else if (text[at] === '.') {
                at += 1;
                if (text[at] === '*') {
                    at += 1;
                    segments.push({ kind: 'all' });
                } else {
                    segments.push({ kind: 'name', key: name() });
                }
            } else if (text[at] === '[') {
                at += 1;
                if (text[at] === '?') {
                    at += 1;
                    segments.push({ kind: legacy ? 'where' : 'select', ...filter() });
                } else if (text[at] === '*') {
                    at += 1;
                    segments.push({ kind: 'all' });
                } else if (text[at] === '"' || text[at] === "'") {
                    segments.push({ kind: 'name', key: quoted() });
                } else if (text[at] === '/') {
                    segments.push({ kind: 'match', pattern: regex() });
                } else {
                    const index = /^-?\d+/.exec(text.slice(at));
                    if (index === null) {
                        fail();
                    }
                    at += index[0].length;
                    segments.push({ kind: 'index', index: Number(index[0]) });
                }
                if (text[at] !== ']') {
                    fail();
                }
                at += 1;
            } else if (text[at] === '=' || text.startsWith('+=', at)) {
                change = assignment();
            } else {
                fail();
            }
        }
    } catch (e) {
        return never;
    }
    if (change !== null && !trusted) {
        return never;
    }
    const lookup = (node, path) => {
        let value = node;
        for (const step of path) {
            if (!isObject(value) || !Object.prototype.hasOwnProperty.call(value, step.key)) {
                return { found: false };
            }
            value = value[step.key];
        }
        return { found: true, value };
    };
    const passes = (segment, node) => {
        const { found, value } = lookup(node, segment.path);
        const result = found && segment.test(value);
        return segment.negated ? !result : result;
    };
    // Each match is the value reached and where it sits: [owner, key, value].
    const children = (node) => (isObject(node) ? Object.keys(node).map((key) => [node, key, node[key]]) : []);
    const descendants = (match, out) => {
        out.push(match);
        for (const child of children(match[2])) {
            descendants(child, out);
        }
        return out;
    };
    const step = (matches, segment) => {
        const out = [];
        for (const match of matches) {
            const node = match[2];
            switch (segment.kind) {
            case 'name':
                if (isObject(node) && Object.prototype.hasOwnProperty.call(node, segment.key)) {
                    out.push([node, segment.key, node[segment.key]]);
                }
                break;
            case 'index':
                if (Array.isArray(node)) {
                    const index = segment.index < 0 ? node.length + segment.index : segment.index;
                    if (index >= 0 && index < node.length) {
                        out.push([node, String(index), node[index]]);
                    }
                }
                break;
            case 'all':
                out.push(...children(node));
                break;
            case 'match':
                out.push(...children(node).filter(([, key]) => segment.pattern.test(key)));
                break;
            case 'descend':
                descendants(match, out);
                break;
            case 'where':
                if (passes(segment, node)) {
                    out.push(match);
                }
                break;
            case 'select':
                out.push(...children(node).filter((child) => passes(segment, child[2])));
                break;
            default:
                break;
            }
        }
        return out;
    };
    // `"${now}"`, as in `.a.b="${now}"` in uBO's lists, is the time of the
    // edit. No wiki page documents it; the rules use it as a timestamp.
    const valueOf = (value) => {
        if (value === '${now}') {
            return String(Date.now());
        }
        return isObject(value) ? JSON.parse(JSON.stringify(value)) : value;
    };
    // Edits `root` in place and says whether anything changed.
    const edit = (root) => {
        if (!isObject(root)) {
            return false;
        }
        const matches = segments.reduce(step, [[null, null, root]]);
        if (matches.length === 0) {
            return false;
        }
        let changed = false;
        // Backwards, so removing an array entry keeps the others' places.
        for (const [owner, key, value] of matches.reverse()) {
            if (change === null) {
                if (owner === null) {
                    continue;
                }
                if (Array.isArray(owner)) {
                    owner.splice(Number(key), 1);
                } else {
                    delete owner[key];
                }
                changed = true;
            } else if (change.kind === 'merge') {
                if (isObject(value)) {
                    Object.assign(value, valueOf(change.value));
                    changed = true;
                }
            } else if (owner !== null) {
                if (change.kind === 'replace') {
                    if (typeof value === 'string') {
                        owner[key] = value.replace(change.find, change.replacement);
                        changed = changed || owner[key] !== value;
                    }
                } else {
                    owner[key] = valueOf(change.value);
                    changed = true;
                }
            }
        }
        return changed;
    };
    return { edit };
}
