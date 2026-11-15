"use strict";
const sharedLexicalFiles = { count: 1 };
globalThis.strictThisUndefined =
  (function () {
    return this;
  })() === undefined;
