// Implements no-xhr-if from uBlock Origin's wiki, Resources Library: "no-xhr-if.js".
function zephiumNoXhrIf(conditions = '', directive = '') {
    if (typeof XMLHttpRequest !== 'function') {
        return;
    }
    const tests = (typeof conditions === 'string' ? conditions.split(/\s+/) : [])
        .filter((token) => token !== '')
        .map((token) => {
            const named = /^(method|url):(.*)$/.exec(token);
            if (named !== null) {
                const test = zephiumPattern(named[2]);
                return (details) => test(details[named[1]]);
            }
            const negated = token.startsWith('!');
            const test = zephiumPattern(negated ? token.slice(1) : token);
            return (details) => test(details.url) !== negated;
        });
    // Without conditions uBlock Origin only logs, which the app does not.
    if (tests.length === 0) {
        return;
    }
    const random = (length) => {
        const letters = 'abcdefghijklmnopqrstuvwxyz0123456789';
        let text = '';
        for (let i = 0; i < length; i++) {
            text += letters[Math.floor(Math.random() * letters.length)];
        }
        return text;
    };
    const body = () => {
        if (directive === 'true') {
            return random(10);
        }
        if (directive === 'emptyObj') {
            return '{}';
        }
        if (directive === 'emptyArr') {
            return '[]';
        }
        const sized = /^length:(\d+)(?:-(\d+))?$/.exec(directive);
        if (sized !== null) {
            const min = Number(sized[1]);
            const max = sized[2] === undefined ? min : Math.max(min, Number(sized[2]));
            return random(Math.min(500000, min + Math.floor(Math.random() * (max - min + 1))));
        }
        // emptyStr, and war: resources the app does not ship.
        return '';
    };
    // open() sends the six methods Fetch normalizes in upper case, others as given.
    const method = (value) => {
        const text = String(value);
        const upper = text.toUpperCase();
        return ['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT'].includes(upper) ? upper : text;
    };
    const later = setTimeout;
    const proto = XMLHttpRequest.prototype;
    const defused = new WeakMap();
    proto.open = new Proxy(proto.open, {
        apply(target, self, args) {
            const details = { method: method(args[0]), url: String(args[1]) };
            if (tests.every((test) => test(details))) {
                defused.set(self, { url: details.url, sync: args[2] === false, sent: false, done: false, text: body() });
            } else {
                defused.delete(self);
            }
            return Reflect.apply(target, self, args);
        },
    });
    if (typeof proto.abort === 'function') {
        proto.abort = new Proxy(proto.abort, {
            apply(target, self, args) {
                defused.delete(self);
                return Reflect.apply(target, self, args);
            },
        });
    }
    // A listener that opens or aborts the request ends the sequence for it.
    const finish = (xhr, entry) => {
        if (defused.get(xhr) !== entry) {
            return;
        }
        entry.done = true;
        for (const type of ['readystatechange', 'load', 'loadend']) {
            if (defused.get(xhr) !== entry) {
                return;
            }
            xhr.dispatchEvent(new Event(type));
        }
    };
    proto.send = new Proxy(proto.send, {
        apply(target, self, args) {
            const entry = defused.get(self);
            if (entry === undefined) {
                return Reflect.apply(target, self, args);
            }
            if (entry.sent) {
                throw new DOMException("Failed to execute 'send' on 'XMLHttpRequest': The object's state must be OPENED.", 'InvalidStateError');
            }
            entry.sent = true;
            // A native request completes from a later task, after the caller's continuation.
            if (entry.sync) {
                finish(self, entry);
            } else {
                later(() => finish(self, entry), 0);
            }
        },
    });
    const decode = (type, text) => {
        switch (type) {
            case '':
            case 'text':
                return text;
            case 'json':
                try {
                    return JSON.parse(text);
                } catch (e) {
                    return null;
                }
            case 'arraybuffer':
                return new TextEncoder().encode(text).buffer;
            case 'blob':
                return new Blob([text]);
            default:
                return null;
        }
    };
    // The same object for repeated reads, until the page asks for another type.
    const response = (xhr, entry) => {
        if (entry.decoded === undefined || entry.decoded.type !== xhr.responseType) {
            entry.decoded = { type: xhr.responseType, value: decode(xhr.responseType, entry.text) };
        }
        return entry.decoded.value;
    };
    const responseText = (xhr, entry) => {
        if (xhr.responseType !== '' && xhr.responseType !== 'text') {
            throw new DOMException("Failed to read the 'responseText' property from 'XMLHttpRequest': The value is only accessible if the object's 'responseType' is '' or 'text'.", 'InvalidStateError');
        }
        return entry.text;
    };
    const answers = {
        readyState: () => 4,
        status: () => 200,
        statusText: () => 'OK',
        responseURL: (xhr, entry) => entry.url,
        responseText,
        response,
    };
    for (const [name, answer] of Object.entries(answers)) {
        const native = Object.getOwnPropertyDescriptor(proto, name);
        Object.defineProperty(proto, name, {
            configurable: true,
            enumerable: true,
            get() {
                const entry = defused.get(this);
                if (entry !== undefined && entry.done) {
                    return answer(this, entry);
                }
                return native?.get ? Reflect.apply(native.get, this, []) : undefined;
            },
        });
    }
}
