// ==UserScript==
// @name Run 5 localhost GM roundtrip
// @namespace zephium-localhost-fixture
// @version 1
// @match http://127.0.0.1/*
// @grant GM.setValue
// @grant GM.getValue
// @run-at document-end
// ==/UserScript==
(async () => {
  await GM.setValue('run5-fixture', 42);
  document.documentElement.dataset.run5UserScript = String(await GM.getValue('run5-fixture'));
})();
