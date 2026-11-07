#!/bin/zsh
# Runs the recorder in real WebKit against a server on 127.0.0.1: one page
# redirects, and another's frame leads, to a host off the reviewed list. Both
# hops must be refused and logged, and nothing from those hosts recorded,
# while the allowed page's own loads still are.
set -euo pipefail
here=${0:A:h}
out=$(mktemp -d)
server=
trap '[[ -n $server ]] && kill $server 2>/dev/null; rm -rf "$out"' EXIT
xcrun swiftc -parse-as-library -O -warnings-as-errors -module-cache-path "$out/cache" \
  -framework WebKit "$here/record.swift" -o "$out/record"
python3 -I - "$out/port" <<'EOF' &
import http.server, sys
pages = {
    "/redirect": (302, {"Location": "http://refused-redirect.test/"}, b""),
    "/framed": (200, {"Content-Type": "text/html"},
                b'<!doctype html><img src="/pixel.gif"><iframe src="http://refused-frame.test/"></iframe>'),
    "/pixel.gif": (200, {"Content-Type": "image/gif"},
                   b"GIF89a\x01\x00\x01\x00\x00\x00\x00;"),
}
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        status, headers, body = pages.get(self.path, (404, {}, b""))
        self.send_response(status)
        for name, value in headers.items():
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args):
        pass
httpd = http.server.HTTPServer(("127.0.0.1", 0), Handler)
open(sys.argv[1], "w").write(str(httpd.server_address[1]))
httpd.serve_forever()
EOF
server=$!
for _ in {1..50}; do [[ -s $out/port ]] && break; sleep 0.1; done
origin="http://127.0.0.1:$(<$out/port)"
print -l "$origin/redirect" "$origin/framed" > "$out/pages.txt"
print 127.0.0.1 > "$out/allowed.txt"
: > "$out/consent.js"
"$out/record" "$out/pages.txt" "$out/corpus.jsonl" "$out/visits.jsonl" 2 \
  "$out/consent.js" "$out/allowed.txt" 2> "$out/log"
fail() { print -u2 "record test: $1"; cat "$out/log" >&2; exit 1; }
grep -qx 'refused main refused-redirect.test' "$out/log" || fail "the redirect hop was not refused"
grep -qx 'refused frame refused-frame.test' "$out/log" || fail "the frame was not refused"
! grep -q 'refused-redirect\.test\|refused-frame\.test' "$out/corpus.jsonl" \
  || fail "a refused host was recorded"
grep -q 'pixel\.gif' "$out/corpus.jsonl" || fail "the allowed page's own load was not recorded"
print "record test: refused hops logged, nothing from them recorded"
