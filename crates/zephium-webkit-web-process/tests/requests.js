async function nativeRequests(source = 'page') {
    return Object.fromEntries(await Promise.all(['cancel', 'redirect', 'headers'].map(async action => {
        const controller = new AbortController();
        const timer = setTimeout(() => controller.abort(), 3000);
        try {
            const response = await fetch(`/policy-${action}?source=${source}&kind=fetch`, {
                cache: 'no-store', signal: controller.signal
            });
            return [action, { body: await response.text() }];
        } catch (error) {
            return [action, { error: error.name }];
        } finally {
            clearTimeout(timer);
        }
    })));
}

async function nativeWorker(kind) {
    return await new Promise(resolve => {
        const worker = new Worker(`/worker.js?kind=${kind}`, kind === 'module' ? { type: 'module' } : {});
        const finish = value => { clearTimeout(timer); worker.terminate(); resolve(value); };
        const timer = setTimeout(() => finish({ error: 'timeout' }), 8000);
        worker.onmessage = event => finish(event.data);
        worker.onerror = event => finish({ error: event.message });
        worker.postMessage(kind);
    });
}

async function nativeSubresources() {
    const results = [];
    for (const kind of ['document', 'script', 'image']) {
        for (const action of ['cancel', 'redirect', 'headers']) {
            results.push(await new Promise(resolve => {
                const element = document.createElement({ document: 'iframe', script: 'script', image: 'img' }[kind]);
                const finish = loaded => {
                    clearTimeout(timer);
                    let body = null;
                    if (kind === 'document') {
                        try { body = element.contentDocument?.body?.textContent.trim(); } catch {}
                    }
                    if (kind === 'script') body = globalThis.nativeScriptResult ?? null;
                    element.remove();
                    resolve({ kind, action, loaded, body });
                };
                const timer = setTimeout(() => finish('timeout'), 4000);
                globalThis.nativeScriptResult = undefined;
                element.onload = () => finish(true);
                element.onerror = () => finish(false);
                element.src = `/policy-${action}?source=page&kind=${kind}`;
                document.body.append(element);
            }));
        }
    }
    return results;
}
