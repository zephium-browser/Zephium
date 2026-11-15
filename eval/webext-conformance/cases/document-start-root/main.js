globalThis.startRoot = {root: document.documentElement !== null, beforePage: !globalThis.pageScriptRan};
Object.defineProperty(globalThis, "pageScriptRan", {configurable:true, get(){return true}, set(){document.documentElement.dataset.mainBeforeInline = String(startRoot.root && startRoot.beforePage)}});
document.documentElement.dataset.mainRoot = "present";
