import AppKit
import Foundation
import WebKit

// Qualification only: never accepts a personal file or browser profile.
// Each run owns its HTML/image fixtures and nonpersistent WebKit stores.
private enum Failure: Error { case check(String) }

private final class Page: NSObject, WKNavigationDelegate {
    let view: WKWebView
    let selected: URL
    var loaded = false
    var failure: String?
    var denied = 0

    init(selected: URL) {
        self.selected = selected
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        view = WKWebView(frame: NSRect(x: 0, y: 0, width: 640, height: 480), configuration: configuration)
        super.init()
        view.navigationDelegate = self
    }

    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        // The prototype owns exactly one local top-level document. No generic
        // file navigation, subframe grant, IPC handler or custom protocol.
        if action.targetFrame?.isMainFrame == true && action.request.url == selected {
            decisionHandler(.allow)
        } else {
            denied += 1
            decisionHandler(.cancel)
        }
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { loaded = true }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        failure = "provisional navigation failed"
    }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        failure = "navigation failed"
    }

    func start(readAccess: URL) throws {
        guard view.loadFileURL(selected, allowingReadAccessTo: readAccess) != nil else {
            throw Failure.check("load was not admitted")
        }
        try pump { self.loaded || self.failure != nil }
        guard loaded && failure == nil && view.url == selected else {
            throw Failure.check("selected document did not commit")
        }
    }

    func startHTML(_ html: String) throws {
        guard view.loadHTMLString(html, baseURL: nil) != nil else {
            throw Failure.check("HTML snapshot was not admitted")
        }
        try pump { self.loaded || self.failure != nil }
        guard loaded && failure == nil else { throw Failure.check("HTML snapshot did not commit") }
    }

    func evaluate(_ script: String, arguments: [String: Any] = [:]) throws -> Any {
        var answer: Result<Any, Error>?
        view.callAsyncJavaScript(script, arguments: arguments, in: nil, in: .page) {
            answer = $0
        }
        try pump { answer != nil }
        return try answer!.get()
    }

    func imageLoads(_ url: URL) throws -> Bool {
        let result = try evaluate("""
            return await new Promise(resolve => {
                const image = new Image();
                const timeout = setTimeout(() => resolve('timeout'), 3000);
                image.onload = () => { clearTimeout(timeout); resolve(true); };
                image.onerror = () => { clearTimeout(timeout); resolve(false); };
                image.src = target + '?probe=' + Math.random();
            });
            """, arguments: ["target": url.absoluteString])
        guard let loaded = result as? Bool else {
            throw Failure.check("image access did not settle")
        }
        return loaded
    }
}

private func pump(until done: () -> Bool) throws {
    let deadline = Date().addingTimeInterval(12)
    while !done() && Date() < deadline {
        _ = RunLoop.main.run(mode: .default, before: Date().addingTimeInterval(0.01))
    }
    guard done() else { throw Failure.check("native deadline exceeded") }
}

private func require(_ condition: Bool, _ message: String) throws {
    guard condition else { throw Failure.check(message) }
    print("PASS: \(message)")
}

@main
private struct LocalNewTabProbe {
    static func main() {
        do { try run() } catch {
            print("FAIL: \(error)")
            exit(1)
        }
    }

    static func run() throws {
        let application = NSApplication.shared
        application.setActivationPolicy(.prohibited)
        let root = URL(fileURLWithPath: FileManager.default.currentDirectoryPath, isDirectory: true)
            .appendingPathComponent("target/local-newtab-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root.deletingLastPathComponent(), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false,
                                                attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: root) }
        let directory = root.appendingPathComponent("selected", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
        let html = directory.appendingPathComponent("home.html")
        try "<html><head><title>Local start page</title></head><body><h1>Local start page</h1><script>globalThis.localStartReady = true</script></body></html>".write(to: html, atomically: true, encoding: .utf8)
        let png = Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aEVsAAAAASUVORK5CYII=")!
        let sibling = directory.appendingPathComponent("sibling.png")
        let outside = root.appendingPathComponent("outside.png")
        try png.write(to: sibling)
        try png.write(to: outside)
        let narrow = Page(selected: html)
        try narrow.start(readAccess: html)
        try require(try narrow.evaluate("return globalThis.localStartReady === true") as? Bool == true,
                    "selected local HTML runs its own script")
        try require(try narrow.evaluate("return typeof window.__TAURI__ === 'undefined' && typeof window.__TAURI_INTERNALS__ === 'undefined' && !window.webkit?.messageHandlers?.ipc") as? Bool == true,
                    "document has no application IPC bridge")
        let siblingWasReadable = try narrow.imageLoads(sibling)
        print("OBSERVED: direct file grant allowed sibling image = \(siblingWasReadable)")
        print("OBSERVED: direct file grant allowed outside image = \(try narrow.imageLoads(outside))")

        // Positive control: the image is valid and WebKit can load it when
        // that directory is explicitly granted. This is not a proposed
        // product permission, and its broader authority is not inherited.
        let broad = Page(selected: html)
        try broad.start(readAccess: directory)
        try require(try broad.imageLoads(sibling), "directory-grant positive control can read sibling")
        print("OBSERVED: directory grant allowed outside image = \(try broad.imageLoads(outside))")
        print("OBSERVED: direct file view reads sibling after another view = \(try narrow.imageLoads(sibling))")

        // Stage only the selected document into a fresh directory. Neither
        // sibling files nor the source directory are granted to this view.
        let stagedDirectory = root.appendingPathComponent("staged", isDirectory: true)
        try FileManager.default.createDirectory(at: stagedDirectory, withIntermediateDirectories: false,
                                                attributes: [.posixPermissions: 0o700])
        let stagedHTML = stagedDirectory.appendingPathComponent("home.html")
        try FileManager.default.copyItem(at: html, to: stagedHTML)
        let staged = Page(selected: stagedHTML)
        try staged.start(readAccess: stagedDirectory)
        try require(try staged.evaluate("return globalThis.localStartReady === true") as? Bool == true,
                    "isolated copy runs the selected HTML")
        print("OBSERVED: staged copy allowed source-directory image = \(try staged.imageLoads(sibling))")
        print("OBSERVED: staged copy allowed outside image = \(try staged.imageLoads(outside))")

        // Alternative: import the document bytes with no file base URL or
        // local-file grant. Relative local assets are intentionally absent.
        let snapshot = Page(selected: URL(string: "about:blank")!)
        try snapshot.startHTML(String(contentsOf: html, encoding: .utf8))
        try require(try snapshot.evaluate("return globalThis.localStartReady === true") as? Bool == true,
                    "opaque HTML snapshot runs the selected script")
        try require(!(try snapshot.imageLoads(sibling)), "opaque HTML snapshot denies source-directory image")
        try require(!(try snapshot.imageLoads(outside)), "opaque HTML snapshot denies outside image")
        try require(try snapshot.evaluate("return typeof window.__TAURI_INTERNALS__ === 'undefined'") as? Bool == true,
                    "opaque HTML snapshot has no application bridge")
        snapshot.view.stopLoading()

        _ = try narrow.evaluate("""
            window.location.href = target;
            await new Promise(resolve => setTimeout(resolve, 500));
            return true;
            """, arguments: ["target": outside.absoluteString])
        try require(narrow.view.url == html, "page-initiated navigation cannot select another local file")
        narrow.view.stopLoading()
        broad.view.stopLoading()
        print("Experiment completed. Product file navigation remains disabled.")
    }
}
