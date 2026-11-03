// Records a request corpus: visits pages in a bare WKWebView with no blocking and writes every
// resource load WebKit checks against content rules (all frames) with its resource type, through
// one notify rule per type. Each page loads, settles, then scrolls four times.
// usage: record <pages.txt> <out.jsonl> [settle-seconds] [consent.js]
import AppKit
import WebKit

let types = ["top-document", "child-document", "image", "style-sheet", "script", "font", "media",
             "svg-document", "ping", "fetch", "websocket", "other"]

final class Recorder: NSObject, WKNavigationDelegate {
    var page = ""
    var finished = false
    var out: FileHandle
    var count = 0
    init(out: FileHandle) { self.out = out }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { finished = true }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { finished = true }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { finished = true }
    @objc(_webView:contentRuleListWithIdentifier:performedAction:forURL:)
    func contentRuleAction(_ webView: WKWebView, identifier: NSString, action: NSObject, url: NSURL?) {
        guard let url = url?.absoluteString, let notes = action.value(forKey: "notifications") as? [String] else { return }
        for type in notes {
            let row: [String: String] = ["page": page, "top": webView.url?.absoluteString ?? page, "url": url, "type": type]
            if let data = try? JSONSerialization.data(withJSONObject: row) { out.write(data); out.write(Data("\n".utf8)) }
            count += 1
        }
    }
}

@MainActor func spin(_ timeout: TimeInterval, until done: () -> Bool) {
    let deadline = Date().addingTimeInterval(timeout)
    while !done() && Date() < deadline {
        if let e = NSApp.nextEvent(matching: .any, until: Date(timeIntervalSinceNow: 0.01), inMode: .default, dequeue: true) { NSApp.sendEvent(e) }
    }
}

@main struct Main {
    @MainActor static func main() {
        let args = CommandLine.arguments
        let pages = try! String(contentsOfFile: args[1], encoding: .utf8).split(separator: "\n").map(String.init).filter { !$0.isEmpty && !$0.hasPrefix("#") }
        FileManager.default.createFile(atPath: args[2], contents: nil)
        let out = FileHandle(forWritingAtPath: args[2])!
        out.seekToEndOfFile()
        let settle = args.count > 3 ? Double(args[3])! : 6
        let consent = args.count > 4 ? try? String(contentsOfFile: args[4], encoding: .utf8) : nil
        _ = NSApplication.shared
        NSApp.setActivationPolicy(.regular)
        NSApp.finishLaunching()
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1280, height: 900), styleMask: [.titled], backing: .buffered, defer: false)
        window.level = .floating
        window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        let rules = types.map { ["trigger": ["url-filter": ".*", "resource-type": [$0]], "action": ["type": "notify", "notification": $0]] }
        let json = String(data: try! JSONSerialization.data(withJSONObject: rules), encoding: .utf8)!
        let store = WKContentRuleListStore(url: FileManager.default.temporaryDirectory.appendingPathComponent("record-store"))!
        var list: WKContentRuleList?; var compiled = false
        store.compileContentRuleList(forIdentifier: "notify-all", encodedContentRuleList: json) { l, e in list = l; compiled = true; if let e { print("compile \(e)") } }
        spin(60) { compiled }
        guard let list else { exit(1) }
        let recorder = Recorder(out: out)
        // One configuration for every page, so they share one process pool (a fresh configuration
        // starts its own, and thousands exhaust the system's power-notification registrations).
        let config = WKWebViewConfiguration()
        config.userContentController.add(list)
        if let consent { config.userContentController.addUserScript(WKUserScript(source: consent, injectionTime: .atDocumentEnd, forMainFrameOnly: true)) }
        config.mediaTypesRequiringUserActionForPlayback = []
        for (n, page) in pages.enumerated() {
            config.websiteDataStore = .nonPersistent()
            let view = WKWebView(frame: window.contentView!.bounds, configuration: config)
            view.customUserAgent = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15"
            window.contentView!.addSubview(view)
            view.navigationDelegate = recorder
            recorder.page = page; recorder.finished = false
            let before = recorder.count
            let start = Date()
            view.load(URLRequest(url: URL(string: page)!))
            spin(25) { recorder.finished }
            spin(settle / 2) { false }
            for _ in 0..<4 {
                view.evaluateJavaScript("window.scrollBy(0, innerHeight * 0.9)") { _, _ in }
                spin(settle / 6) { false }
            }
            FileHandle.standardError.write("\(n + 1)/\(pages.count) \(page) \(recorder.count - before) loads \(Int(Date().timeIntervalSince(start)))s\n".data(using: .utf8)!)
            view.navigationDelegate = nil
            view.removeFromSuperview()
        }
        out.closeFile()
    }
}
