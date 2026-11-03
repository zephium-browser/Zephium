// Checks the built-in WebExtension that carries the scriptlet program: a MAIN-world, all-frames,
// document_start content script runs before page scripts in the top frame and a cross-origin
// frame; denying a host pattern stops it on the next load of that host in the same view and
// clearing the denial restores it; it runs in a private data store with private access granted;
// and rewriting the program and reloading the context leaves open pages alone.
// usage: scripts/blocker-scriptlets/run.sh
import AppKit
import Network
import WebKit

@MainActor func spin(_ timeout: TimeInterval, until done: () -> Bool) {
    let deadline = Date().addingTimeInterval(timeout)
    while !done() && Date() < deadline {
        if let e = NSApp.nextEvent(matching: .any, until: Date(timeIntervalSinceNow: 0.01), inMode: .default, dequeue: true) { NSApp.sendEvent(e) }
    }
}

final class Probe: NSObject, WKScriptMessageHandler, WKNavigationDelegate {
    var reports: [String] = []
    var finished = false
    func userContentController(_ c: WKUserContentController, didReceive message: WKScriptMessage) { reports.append("\(message.body)") }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { finished = true }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { finished = true }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { finished = true }
}

@main struct Main {
    @MainActor static func main() {
        guard #available(macOS 15.4, *) else {
            print("skipped: WKWebExtension needs macOS 15.4")
            return
        }
        _ = NSApplication.shared
        NSApp.setActivationPolicy(.regular)
        NSApp.finishLaunching()
        // Two origins on one local listener (its own queue: the main queue is busy spinning).
        let queue = DispatchQueue(label: "scriptlet-spike-server")
        let listener = try! NWListener(using: .tcp, on: .any)
        listener.newConnectionHandler = { connection in
            connection.start(queue: queue)
            connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) { data, _, _, _ in
                let request = String(decoding: data ?? Data(), as: UTF8.self)
                let path = request.split(separator: " ").dropFirst().first.map(String.init) ?? "/"
                let port = listener.port!.rawValue
                // The first page script reports what the content script left before any page code.
                let report = "<script>webkit.messageHandlers.probe.postMessage(location.hostname + ' ' + (window.top === window ? 'top' : 'frame') + ' runs=' + (window.__zephium || 0))</script>"
                let body = path == "/frame"
                    ? "<!doctype html><html><head>\(report)</head><body></body></html>"
                    : "<!doctype html><html><head>\(report)</head><body><iframe src=\"http://127.0.0.1:\(port)/frame\"></iframe></body></html>"
                let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: \(body.utf8.count)\r\nConnection: close\r\n\r\n\(body)"
                connection.send(content: Data(response.utf8), completion: .contentProcessed { _ in connection.cancel() })
            }
        }
        listener.start(queue: queue)
        spin(5) { (listener.port?.rawValue ?? 0) != 0 }
        let port = listener.port!.rawValue
        // A private copy, so the update step does not rewrite the checked-in program.
        let source = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("scriptlet-spike-\(getpid())")
        try? FileManager.default.removeItem(at: directory)
        try! FileManager.default.copyItem(at: source, to: directory)
        defer { try? FileManager.default.removeItem(at: directory) }
        func load() -> WKWebExtension {
            var loaded: WKWebExtension?
            WKWebExtension.__extension(withResourceBaseURL: directory) { ext, error in
                if let error { print("extension error: \(error)") }
                loaded = ext
            }
            spin(10) { loaded != nil }
            return loaded!
        }
        let controller = WKWebExtensionController()
        func context(_ webExtension: WKWebExtension) -> WKWebExtensionContext {
            let context = WKWebExtensionContext(for: webExtension)
            context.setPermissionStatus(.grantedExplicitly, for: WKWebExtension.MatchPattern.allURLs())
            context.hasAccessToPrivateData = true
            return context
        }
        var loaded = context(load())
        try! controller.load(loaded)
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 800, height: 600), styleMask: [.titled], backing: .buffered, defer: false)
        window.makeKeyAndOrderFront(nil)
        func view(privateStore: Bool) -> (WKWebView, Probe) {
            let probe = Probe()
            let config = WKWebViewConfiguration()
            config.webExtensionController = controller
            if privateStore { config.websiteDataStore = .nonPersistent() }
            config.userContentController.add(probe, name: "probe")
            let view = WKWebView(frame: window.contentView!.bounds, configuration: config)
            view.navigationDelegate = probe
            window.contentView!.addSubview(view)
            return (view, probe)
        }
        var failures = 0
        func visit(_ view: WKWebView, _ probe: Probe, _ host: String, expect: [String], _ label: String) {
            probe.reports = []; probe.finished = false
            view.load(URLRequest(url: URL(string: "http://\(host):\(port)/")!))
            spin(10) { probe.finished && probe.reports.count >= 2 }
            spin(0.3) { false }
            let got = probe.reports.sorted()
            let ok = got == expect.sorted()
            if !ok { failures += 1 }
            print("\(ok ? "ok  " : "FAIL") \(label): \(got.joined(separator: " | "))")
        }
        let (tab, probe) = view(privateStore: false)
        visit(tab, probe, "localhost", expect: ["localhost top runs=1", "127.0.0.1 frame runs=1"], "runs before page scripts, top and cross-origin frame")
        let paused = try! WKWebExtension.MatchPattern(string: "*://localhost/*")
        loaded.setPermissionStatus(.deniedExplicitly, for: paused)
        visit(tab, probe, "localhost", expect: ["localhost top runs=0", "127.0.0.1 frame runs=1"], "paused host, same view")
        visit(tab, probe, "127.0.0.1", expect: ["127.0.0.1 top runs=1", "127.0.0.1 frame runs=1"], "other host, same view")
        loaded.setPermissionStatus(.unknown, for: paused)
        visit(tab, probe, "localhost", expect: ["localhost top runs=1", "127.0.0.1 frame runs=1"], "unpaused, same view")
        let (privateTab, privateProbe) = view(privateStore: true)
        visit(privateTab, privateProbe, "localhost", expect: ["localhost top runs=1", "127.0.0.1 frame runs=1"], "private data store")
        let started = Date()
        try! controller.unload(loaded)
        try! "window.__zephium = 100;\n".write(to: directory.appendingPathComponent("program.js"), atomically: true, encoding: .utf8)
        loaded = context(load())
        try! controller.load(loaded)
        print(String(format: "context reload: %.0f ms", Date().timeIntervalSince(started) * 1000))
        let before = probe.reports.count
        spin(1) { false }
        if probe.reports.count != before { failures += 1 }
        print("\(probe.reports.count == before ? "ok  " : "FAIL") open page untouched by the reload")
        visit(tab, probe, "localhost", expect: ["localhost top runs=100", "127.0.0.1 frame runs=100"], "next load runs the new program")
        print(failures == 0 ? "all checks passed" : "\(failures) checks failed")
        exit(failures == 0 ? 0 : 1)
    }
}
