// Records a request corpus: visits pages in a bare WKWebView with no blocking and writes every
// resource load WebKit checks against content rules (all frames) with its resource type, through
// one notify rule per type. Each page loads, settles, then scrolls four times, and its visit is
// written with a status: ok, failed, timeout (never finished) or late (finished after the wait).
// A page or frame navigation, and each redirect it takes, goes ahead only to a host on the
// reviewed list (matched exactly, ignoring case); any other is cancelled and logged as
// `refused <main|frame> <host>`. Subresource loads are not navigations and are recorded.
// usage: record <pages.txt> <out.jsonl> <visits.jsonl> <settle-seconds> <consent.js> <allowed-hosts.txt>
import AppKit
import WebKit

let types = ["top-document", "child-document", "image", "style-sheet", "script", "font", "media",
             "svg-document", "ping", "fetch", "websocket", "other"]

/// The key WebKit wraps a page's stored CryptoKeys with, for this run only. Unanswered, WebKit keeps
/// one in the login keychain, which prompts for every new build of a tool.
let runKey: NSData = {
    var bytes = [UInt8](repeating: 0, count: 32)
    guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else {
        FileHandle.standardError.write("no random bytes for the WebCrypto key\n".data(using: .utf8)!)
        exit(3)
    }
    return Data(bytes) as NSData
}()

/// Answers WebKit's requests for that key: a page's, through its navigation delegate, and a
/// worker's, through its data store's delegate.
class KeyedDelegate: NSObject {
    @objc(_webCryptoMasterKeyForWebView:)
    func webCryptoMasterKey(_ webView: WKWebView) -> NSData { asked("page"); return runKey }
    @objc(_webCryptoMasterKeyForWebView:completionHandler:)
    func webCryptoMasterKey(_ webView: WKWebView, completionHandler: @escaping (NSData?) -> Void) { asked("page"); completionHandler(runKey) }
    @objc(webCryptoMasterKey:)
    func webCryptoMasterKey(_ completionHandler: @escaping (NSData?) -> Void) { asked("data store"); completionHandler(runKey) }
    func asked(_ by: String) { FileHandle.standardError.write("webcrypto key: in memory, for the \(by)\n".data(using: .utf8)!) }
}

/// `store`, with `delegate` answering its workers' requests for the key.
func keyed(_ store: WKWebsiteDataStore, by delegate: KeyedDelegate) -> WKWebsiteDataStore {
    let setter = Selector(("set_delegate:"))
    if store.responds(to: setter) { _ = store.perform(setter, with: delegate) }
    return store
}

final class Recorder: KeyedDelegate, WKNavigationDelegate {
    var page = ""
    var finished = false
    var finishedAt: Date?
    var failed = false
    var out: FileHandle
    var count = 0
    let allowed: Set<String>
    init(out: FileHandle, allowed: Set<String>) { self.out = out; self.allowed = allowed }
    /// Only http(s) leaves the machine; about:blank, data: and the like stay.
    func admitted(_ url: URL?) -> Bool {
        guard let url, let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https" else { return true }
        return url.host.map { allowed.contains($0.lowercased()) } ?? false
    }
    func refuse(_ url: URL?, frame: Bool) {
        FileHandle.standardError.write("refused \(frame ? "frame" : "main") \(url?.host ?? "?")\n".data(using: .utf8)!)
    }
    // Asked for every page and frame navigation, and again for each server redirect.
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        if admitted(action.request.url) { decisionHandler(.allow); return }
        refuse(action.request.url, frame: action.targetFrame?.isMainFrame == false)
        decisionHandler(.cancel)
    }
    // A main-frame server redirect is also checked here, in case the policy was not asked.
    func webView(_ webView: WKWebView, didReceiveServerRedirectForProvisionalNavigation navigation: WKNavigation!) {
        if !admitted(webView.url) {
            refuse(webView.url, frame: false)
            webView.stopLoading()
        }
    }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { finished = true; finishedAt = Date(); failed = false }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { fail(error) }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { fail(error) }
    // A navigation the page replaces (a redirect by script) is cancelled, not failed.
    func fail(_ error: Error) {
        finished = true
        if (error as NSError).code != NSURLErrorCancelled { failed = true }
    }
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
        FileManager.default.createFile(atPath: args[3], contents: nil)
        let visits = FileHandle(forWritingAtPath: args[3])!
        let settle = args.count > 4 ? Double(args[4])! : 6
        let consent = args.count > 5 ? try? String(contentsOfFile: args[5], encoding: .utf8) : nil
        guard args.count > 6, let hostText = try? String(contentsOfFile: args[6], encoding: .utf8) else {
            FileHandle.standardError.write("record needs the reviewed host list\n".data(using: .utf8)!)
            exit(2)
        }
        let allowed = Set(hostText.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces).lowercased() }.filter { !$0.isEmpty && !$0.hasPrefix("#") })
        _ = NSApplication.shared
        NSApp.setActivationPolicy(.accessory)
        NSApp.finishLaunching()
        // Off every screen and never activated, so nothing shows in front of the user; each view is
        // told its window is not occluded, so pages run as in view.
        let window = NSWindow(contentRect: NSRect(x: -20000, y: -20000, width: 1280, height: 900), styleMask: [.borderless], backing: .buffered, defer: false)
        window.orderBack(nil)
        let rules = types.map { ["trigger": ["url-filter": ".*", "resource-type": [$0]], "action": ["type": "notify", "notification": $0]] }
        let json = String(data: try! JSONSerialization.data(withJSONObject: rules), encoding: .utf8)!
        let store = WKContentRuleListStore(url: FileManager.default.temporaryDirectory.appendingPathComponent("record-store"))!
        var list: WKContentRuleList?; var compiled = false
        store.compileContentRuleList(forIdentifier: "notify-all", encodedContentRuleList: json) { l, e in list = l; compiled = true; if let e { print("compile \(e)") } }
        spin(60) { compiled }
        guard let list else { exit(1) }
        let recorder = Recorder(out: out, allowed: allowed)
        // One configuration for every page, so they share one process pool (a fresh configuration
        // starts its own, and thousands exhaust the system's power-notification registrations).
        let config = WKWebViewConfiguration()
        config.userContentController.add(list)
        if let consent { config.userContentController.addUserScript(WKUserScript(source: consent, injectionTime: .atDocumentEnd, forMainFrameOnly: true)) }
        config.mediaTypesRequiringUserActionForPlayback = []
        for (n, page) in pages.enumerated() {
            config.websiteDataStore = keyed(.nonPersistent(), by: recorder)
            let view = WKWebView(frame: window.contentView!.bounds, configuration: config)
            unoccluded(view)
            view.customUserAgent = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15"
            window.contentView!.addSubview(view)
            view.navigationDelegate = recorder
            recorder.page = page; recorder.finished = false; recorder.finishedAt = nil; recorder.failed = false
            let before = recorder.count
            let start = Date()
            view.load(URLRequest(url: URL(string: page)!))
            spin(25) { recorder.finished }
            spin(settle / 2) { false }
            for _ in 0..<4 {
                view.evaluateJavaScript("window.scrollBy(0, innerHeight * 0.9)") { _, _ in }
                spin(settle / 6) { false }
            }
            let status = recorder.failed ? "failed"
                : recorder.finishedAt.map { $0.timeIntervalSince(start) <= 25 ? "ok" : "late" } ?? "timeout"
            let visit = ["page": page, "status": status, "loads": String(recorder.count - before)]
            visits.write(try! JSONSerialization.data(withJSONObject: visit)); visits.write(Data("\n".utf8))
            FileHandle.standardError.write("\(n + 1)/\(pages.count) \(page) \(status) \(recorder.count - before) loads \(Int(Date().timeIntervalSince(start)))s\n".data(using: .utf8)!)
            view.navigationDelegate = nil
            view.removeFromSuperview()
        }
        out.closeFile()
        visits.closeFile()
    }
}
