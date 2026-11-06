// Replays recorded loads through WebKit: each (top page, URL, resource type) is requested again
// from a document whose URL is the original top page, with the given content rule lists installed,
// and the rule notifications WebKit reports for it are written out. Every load goes to a local
// listener that closes the connection unanswered, so no request leaves the machine.
// usage: replay <rows.jsonl> <out.jsonl> <rules.json>...
// Loads of one page go in mixed-type batches; a URL repeats only across batches. A batch whose
// script never reports back may not have issued its loads, so any such batch fails the replay.
import AppKit
import Network
import WebKit

final class Probe: NSObject, WKNavigationDelegate, WKScriptMessageHandler {
    // WebKit asks for the key it wraps a page's stored CryptoKeys with; unanswered, it keeps one in
    // the login keychain, which prompts for a new build. A key for this run only, never stored.
    let masterKey: Data = { var bytes = [UInt8](repeating: 0, count: 32); _ = SecRandomCopyBytes(kSecRandomDefault, 32, &bytes); return Data(bytes) }()
    @objc(_webCryptoMasterKeyForWebView:)
    func webCryptoMasterKey(_ webView: WKWebView) -> NSData { webCryptoKeyAsked(); return masterKey as NSData }
    @objc(_webCryptoMasterKeyForWebView:completionHandler:)
    func webCryptoMasterKey(_ webView: WKWebView, completionHandler: @escaping (NSData?) -> Void) { webCryptoKeyAsked(); completionHandler(masterKey as NSData) }
    func webCryptoKeyAsked() { FileHandle.standardError.write("webcrypto key: in memory\n".data(using: .utf8)!) }
    var blocked = Set<String>()
    var seen: [String: Set<String>] = [:]
    var done = false
    var finished = false
    func userContentController(_ c: WKUserContentController, didReceive message: WKScriptMessage) { done = true }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { finished = true }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { finished = true }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { finished = true }
    @objc(_webView:contentRuleListWithIdentifier:performedAction:forURL:)
    func contentRuleAction(_ webView: WKWebView, identifier: NSString, action: NSObject, url: NSURL?) {
        guard let url = url?.absoluteString else { return }
        if (action.value(forKey: "blockedLoad") as? Bool) == true { blocked.insert(url) }
        for note in (action.value(forKey: "notifications") as? [String]) ?? [] { seen[url, default: []].insert(note) }
    }
}

/// Tells a view its window is never occluded, so its page runs as in view off screen.
func unoccluded(_ view: WKWebView) {
    let setter = Selector(("_setWindowOcclusionDetectionEnabled:"))
    guard view.responds(to: setter) else { return }
    typealias Set = @convention(c) (AnyObject, Selector, Bool) -> Void
    unsafeBitCast(view.method(for: setter), to: Set.self)(view, setter, false)
}

@MainActor func spin(_ timeout: TimeInterval, until done: () -> Bool) {
    let deadline = Date().addingTimeInterval(timeout)
    while !done() && Date() < deadline {
        if let e = NSApp.nextEvent(matching: .any, until: Date(timeIntervalSinceNow: 0.005), inMode: .default, dequeue: true) { NSApp.sendEvent(e) }
    }
}

func request(_ type: String) -> String {
    switch type {
    case "image": return "u => { const i = new Image(); i.src = u; keep.push(i); }"
    case "script": return "u => { const s = document.createElement('script'); s.async = true; s.src = u; document.head.append(s); }"
    case "style-sheet": return "u => { const l = document.createElement('link'); l.rel = 'stylesheet'; l.href = u; document.head.append(l); }"
    case "font": return "u => { new FontFace('f' + keep.length, 'url(' + JSON.stringify(u) + ')').load().catch(() => {}); keep.push(u); }"
    case "media": return "u => { const v = document.createElement('video'); v.preload = 'auto'; v.src = u; keep.push(v); }"
    case "ping", "other": return "u => { const x = new URL(u); if (x.pathname === '/' && !x.search) { const l = document.createElement('link'); l.rel = 'preconnect'; l.href = u; document.head.append(l); } else if (\(type == "ping")) { const a = document.createElement('a'); a.href = '#'; a.ping = u; document.body.append(a); a.click(); } else { const l = document.createElement('link'); l.rel = 'preload'; l.as = 'fetch'; l.crossOrigin = 'anonymous'; l.href = u; document.head.append(l); } }"
    case "fetch": return "u => { fetch(u, { mode: 'no-cors' }).catch(() => {}); }"
    case "websocket": return "u => { try { keep.push(new WebSocket(u)); } catch {} }"
    case "child-document", "svg-document": return "u => { const f = document.createElement('iframe'); f.src = u; document.body.append(f); }"
    default: return "u => { const l = document.createElement('link'); l.rel = 'prefetch'; l.href = u; document.head.append(l); }"
    }
}

@main struct Main {
    @MainActor static func main() {
        let args = CommandLine.arguments
        var unique = Set<String>()
        let rows = try! String(contentsOfFile: args[1], encoding: .utf8).split(separator: "\n").compactMap {
            try? JSONSerialization.jsonObject(with: Data($0.utf8)) as? [String: String]
        }.filter { unique.insert([$0["top"]!, $0["url"]!, $0["type"]!].joined(separator: "\u{1}")).inserted }
        FileManager.default.createFile(atPath: args[2], contents: nil)
        let out = FileHandle(forWritingAtPath: args[2])!
        let listener = try! NWListener(using: .tcp, on: .any)
        listener.newConnectionHandler = { connection in connection.cancel() }
        listener.start(queue: .main)
        _ = NSApplication.shared
        NSApp.setActivationPolicy(.accessory)
        NSApp.finishLaunching()
        // Off every screen and never activated, so nothing shows in front of the user; each view is
        // told its window is not occluded, so pages run as in view.
        let window = NSWindow(contentRect: NSRect(x: -20000, y: -20000, width: 800, height: 600), styleMask: [.borderless], backing: .buffered, defer: false)
        window.orderBack(nil)
        var lists: [WKContentRuleList] = []
        let ruleStore = WKContentRuleListStore(url: FileManager.default.temporaryDirectory.appendingPathComponent("verdict-store"))!
        for path in args[3...] {
            var compiled = false
            ruleStore.compileContentRuleList(forIdentifier: (path as NSString).lastPathComponent, encodedContentRuleList: try! String(contentsOfFile: path, encoding: .utf8)) { l, e in if let l { lists.append(l) }; compiled = true; if let e { print("compile \(e)") } }
            spin(300) { compiled }
        }
        spin(5) { listener.port != nil }
        let store = WKWebsiteDataStore.nonPersistent()
        let proxy = NWEndpoint.hostPort(host: "127.0.0.1", port: listener.port!)
        store.proxyConfigurations = [ProxyConfiguration(httpCONNECTProxy: proxy), ProxyConfiguration(socksv5Proxy: proxy)]
        let probe = Probe()
        // One configuration for every view: each fresh configuration starts its own process pool,
        // and thousands of pools exhaust the system's power-notification registrations.
        let config = WKWebViewConfiguration()
        config.websiteDataStore = store
        for list in lists { config.userContentController.add(list) }
        config.userContentController.add(probe, name: "done")
        func view() -> WKWebView {
            let v = WKWebView(frame: window.contentView!.bounds, configuration: config)
            unoccluded(v)
            v.navigationDelegate = probe
            window.contentView!.addSubview(v)
            return v
        }
        func write(_ row: [String: String], _ blocked: Bool) {
            var r = row; r["blocked"] = blocked ? "1" : "0"
            if let types = probe.seen[row["url"]!] { r["seen"] = types.sorted().joined(separator: ",") }
            out.write(try! JSONSerialization.data(withJSONObject: r)); out.write(Data("\n".utf8))
        }
        var groups: [String: [[String: String]]] = [:]
        var order: [String] = []
        for row in rows {
            let key = row["type"] == "top-document" ? row["top"]! + "\u{1}top" : row["top"]!
            if groups[key] == nil { order.append(key) }
            groups[key, default: []].append(row)
        }
        let goers = ["image", "script", "style-sheet", "font", "media", "ping", "other", "fetch", "websocket", "child-document", "svg-document", "none"]
            .map { "\"\($0)\": \(request($0))" }.joined(separator: ", ")
        var done = 0, stalled = 0
        for key in order {
            let group = groups[key]!
            let type = group[0]["type"]!, top = group[0]["top"]!
            if type == "top-document" {
                let batch = group
                for row in batch {
                    let v = view(); probe.blocked = []; probe.seen = [:]; probe.finished = false
                    v.load(URLRequest(url: URL(string: row["url"]!)!))
                    spin(10) { probe.finished }
                    spin(0.2) { false }
                    write(row, probe.blocked.contains(row["url"]!))
                    v.removeFromSuperview()
                }
                done += batch.count
                continue
            }
            var parts: [[[String: String]]] = []
            for row in group {
                if let i = parts.firstIndex(where: { $0.count < 300 && !$0.contains { $0["url"] == row["url"] } }) { parts[i].append(row) } else { parts.append([row]) }
            }
            for part in parts {
                let pairs = String(data: try! JSONSerialization.data(withJSONObject: part.map { [$0["url"]!, $0["type"]!] }), encoding: .utf8)!
                let html = "<!doctype html><body><script>const keep = []; const go = { \(goers) }; for (const [u, t] of \(pairs)) { try { (go[t] || go.none)(u); } catch {} } setTimeout(() => webkit.messageHandlers.done.postMessage(1), 600);</script></body>"
                let v = view(); probe.blocked = []; probe.seen = [:]; probe.done = false
                v.loadHTMLString(html, baseURL: URL(string: top))
                spin(20) { probe.done }
                if !probe.done { stalled += 1 }
                spin(0.2) { false }
                for row in part { write(row, probe.blocked.contains(row["url"]!)) }
                v.removeFromSuperview()
            }
            done += group.count
            if done % 5000 < group.count { FileHandle.standardError.write("\(done)/\(rows.count)\n".data(using: .utf8)!) }
        }
        out.closeFile()
        if stalled > 0 {
            FileHandle.standardError.write("\(stalled) batches never reported back\n".data(using: .utf8)!)
            exit(1)
        }
    }
}
