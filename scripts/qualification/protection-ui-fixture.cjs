// Local-only UI fixture. Run with Node; no CDP or browser automation bridge.
const http = require('node:http');
const strict = '<!doctype html><title>Protection UI strict CSP</title><h1>Protection UI fixture</h1><p>Useful content must remain visible.</p><div id="first"><h2>First target</h2><p>Click this box to hide it.</p></div><div id="second"><h2>Second target</h2><p>Hide this too, then Undo.</p></div><div id="third"><h2>Third target</h2><p>Hide and restart to verify persistence.</p></div>';
const server = http.createServer((request, response) => {
  const path = new URL(request.url, 'http://localhost').pathname;
  console.log(JSON.stringify({ at: Date.now(), path }));
  response.setHeader('Cache-Control', 'no-store');
  if (path === '/ads/cbr.js') {
    response.setHeader('Content-Type', 'application/javascript');
    response.end('document.getElementById("network").textContent="Ad script delivered";');
    return;
  }
  response.setHeader('Content-Type', 'text/html');
  if (path === '/strict') {
    response.setHeader('Content-Security-Policy', "script-src 'none'; style-src 'none'; require-trusted-types-for 'script'");
    response.end(strict);
    return;
  }
  response.end(strict.replace('strict CSP', 'network') + '<p id="network">Ad script blocked (or pending)</p><script src="/ads/cbr.js"></script>');
});
server.listen(0, '127.0.0.1', () => console.log(`FIXTURE http://127.0.0.1:${server.address().port}`));
