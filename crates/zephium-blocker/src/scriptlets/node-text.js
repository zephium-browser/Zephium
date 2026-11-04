// Shared by the node-text scriptlets: the node name, pattern and tokens uBlock Origin's Resources Library documents for "replace-node-text.js" and "remove-node-text.js".
function zephiumNodeText(nodeName, pattern, edit, tokens) {
    if (typeof nodeName !== 'string' || nodeName === '' || typeof pattern !== 'string' || pattern === '') {
        return;
    }
    const settings = {};
    for (let i = 0; i + 1 < tokens.length; i += 2) {
        settings[tokens[i]] = tokens[i + 1];
    }
    const name = nodeName.toUpperCase();
    const found = zephiumPattern(pattern);
    const includes = zephiumPattern(settings.includes ?? settings.condition ?? '');
    const excludes = settings.excludes === undefined || settings.excludes === '' ? null : zephiumPattern(settings.excludes);
    const limit = Math.max(0, parseInt(settings.sedCount, 10) || 0);
    const stay = settings.stay === '1';
    const quitAfter = Math.max(0, parseInt(settings.quitAfter, 10) || 0);
    const seen = new WeakSet();
    let count = 0;
    let failures = 0;
    let observer = null;
    // Pages that require Trusted Types refuse plain script text, so script
    // text goes through a policy of the scriptlet's own.
    let policy;
    const scriptText = (text) => {
        if (policy === undefined) {
            try {
                policy = window.trustedTypes?.createPolicy?.('zephium-node-text', { createScript: (value) => value }) ?? null;
            } catch (e) {
                policy = null;
            }
        }
        return policy === null ? text : policy.createScript(text);
    };
    const textOf = (node) => (node.nodeType === Node.TEXT_NODE ? node.nodeValue : node.textContent) ?? '';
    const visit = (node) => {
        if (node === null || node.nodeName.toUpperCase() !== name || seen.has(node)) {
            return;
        }
        const before = textOf(node);
        if (before === '' || !found(before) || !includes(before) || (excludes !== null && excludes(before))) {
            return;
        }
        seen.add(node);
        const after = edit(before);
        if (after === before) {
            return;
        }
        // A write the page refuses, such as script text without a policy,
        // leaves the node as it was; repeated refusals end the scriptlet.
        try {
            if (node.nodeType === Node.TEXT_NODE) {
                node.nodeValue = after;
            } else {
                node.textContent = name === 'SCRIPT' ? scriptText(after) : after;
            }
        } catch (e) {
            failures += 1;
            if (failures >= 3) {
                stop();
            }
            return;
        }
        count += 1;
        if (limit !== 0 && count >= limit) {
            stop();
        }
    };
    const stop = () => {
        if (observer !== null) {
            observer.disconnect();
            observer = null;
        }
    };
    // A parser-inserted script's text arrives as its own child, so the
    // element is checked again when text is added under it.
    const added = (node) => {
        visit(node);
        if (node.nodeType === Node.TEXT_NODE) {
            visit(node.parentNode);
        } else if (node.nodeType === Node.ELEMENT_NODE && node.firstChild !== null) {
            const walker = document.createTreeWalker(node, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT);
            while (walker.nextNode() !== null && observer !== null) {
                visit(walker.currentNode);
            }
        }
    };
    observer = new MutationObserver((records) => {
        for (const record of records) {
            for (const node of record.addedNodes) {
                if (observer === null) {
                    return;
                }
                added(node);
            }
        }
    });
    observer.observe(document, { childList: true, subtree: true });
    if (document.documentElement !== null) {
        added(document.documentElement);
    }
    if (stay) {
        return;
    }
    const quit = () => {
        if (quitAfter === 0) {
            stop();
        } else {
            setTimeout(stop, quitAfter);
        }
    };
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', quit, { once: true });
    } else {
        quit();
    }
}
