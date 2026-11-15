"use strict";
const firstRoot = document.documentElement;
globalThis.startRoot = {root: firstRoot !== null, beforePage: !firstRoot?.dataset.pageScript, isolated: typeof globalThis.pageScriptRan === "undefined"};
