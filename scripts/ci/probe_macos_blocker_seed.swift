import CryptoKit
import Darwin
import Foundation
import WebKit

private let maximumArtifactBytes: Int64 = 32 * 1024 * 1024
private let nativeDeadlineSeconds: TimeInterval = 170
private let productionColdCompileBudgetSeconds: TimeInterval = 15
private let duplicateCallbackDrainSeconds: TimeInterval = 0.25
private let identifierPrefix = "app.zephium.rules.v1."
private let artifactDigestDomain = Data("zephium-webkit-content-rules".utf8)
private let artifactFormatVersion: UInt32 = 4
private let expectedReleaseArtifactDigest =
    "93f6d4c6233b2e72fcf71cb554ed625fdaf25ac48e464f28c7b69a76f36b6ed5"

private enum ProbeFailure: Error, CustomStringConvertible {
    case message(String)

    var description: String {
        switch self {
        case let .message(message):
            return message
        }
    }
}

private func fail(_ message: String) -> ProbeFailure {
    .message(message)
}

private func posixFailure(_ operation: String) -> ProbeFailure {
    fail("\(operation): \(String(cString: strerror(errno)))")
}

private func readExactRegularArtifact(at path: String) throws -> Data {
    let descriptor = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK)
    guard descriptor >= 0 else {
        throw posixFailure("cannot open blocker artifact")
    }
    defer {
        _ = close(descriptor)
    }

    var before = stat()
    guard fstat(descriptor, &before) == 0 else {
        throw posixFailure("cannot inspect blocker artifact")
    }
    guard (before.st_mode & S_IFMT) == S_IFREG else {
        throw fail("blocker artifact is not a regular file")
    }
    guard before.st_nlink == 1 else {
        throw fail("blocker artifact must have exactly one filesystem link")
    }
    guard before.st_size > 0, before.st_size <= maximumArtifactBytes else {
        throw fail(
            "blocker artifact size \(before.st_size) is outside 1...\(maximumArtifactBytes) bytes"
        )
    }

    var artifact = Data()
    artifact.reserveCapacity(Int(before.st_size))
    var buffer = [UInt8](repeating: 0, count: 64 * 1024)
    while artifact.count <= maximumArtifactBytes {
        let count = read(descriptor, &buffer, buffer.count)
        if count < 0 {
            if errno == EINTR {
                continue
            }
            throw posixFailure("cannot read blocker artifact")
        }
        if count == 0 {
            break
        }
        guard artifact.count + count <= maximumArtifactBytes else {
            throw fail("blocker artifact grew beyond the fixed byte budget while reading")
        }
        artifact.append(buffer, count: count)
    }

    var after = stat()
    guard fstat(descriptor, &after) == 0 else {
        throw posixFailure("cannot re-inspect blocker artifact")
    }
    guard
        after.st_dev == before.st_dev,
        after.st_ino == before.st_ino,
        after.st_mode == before.st_mode,
        after.st_nlink == before.st_nlink,
        after.st_size == before.st_size,
        after.st_mtimespec.tv_sec == before.st_mtimespec.tv_sec,
        after.st_mtimespec.tv_nsec == before.st_mtimespec.tv_nsec,
        after.st_ctimespec.tv_sec == before.st_ctimespec.tv_sec,
        after.st_ctimespec.tv_nsec == before.st_ctimespec.tv_nsec,
        artifact.count == Int(before.st_size)
    else {
        throw fail("blocker artifact changed while it was being admitted")
    }
    return artifact
}

private func isolatedCacheDirectory() throws -> URL {
    let parent = FileManager.default.temporaryDirectory.path
    var template = Array("\(parent)/zephium-wk-rule-probe.XXXXXX".utf8CString)
    let path = template.withUnsafeMutableBufferPointer { buffer -> String? in
        guard let baseAddress = buffer.baseAddress, mkdtemp(baseAddress) != nil else {
            return nil
        }
        return String(cString: baseAddress)
    }
    guard let path else {
        throw posixFailure("cannot create isolated WebKit cache")
    }
    guard chmod(path, S_IRWXU) == 0 else {
        let error = posixFailure("cannot restrict isolated WebKit cache")
        _ = rmdir(path)
        throw error
    }
    return URL(fileURLWithPath: path, isDirectory: true)
}

private final class NativeRuleStoreProbe {
    private enum Phase {
        case idle
        case compiling
        case lookingUp
        case terminal
    }

    private let cacheURL: URL
    private let encodedRules: String
    private let expectedIdentifier: String
    private var phase = Phase.idle
    private var compileStore: WKContentRuleListStore?
    private var lookupStore: WKContentRuleListStore?
    private var compileStartedAtNanoseconds: UInt64?
    private var lookupStartedAtNanoseconds: UInt64?

    private(set) var terminalResult: Result<Void, ProbeFailure>?
    private(set) var duplicateOrOutOfOrderCallback = false
    private(set) var coldCompileSeconds: TimeInterval?
    private(set) var cacheLookupSeconds: TimeInterval?

    init(cacheURL: URL, encodedRules: String, expectedIdentifier: String) {
        self.cacheURL = cacheURL
        self.encodedRules = encodedRules
        self.expectedIdentifier = expectedIdentifier
    }

    func start() {
        guard phase == .idle else {
            finish(.failure(fail("native blocker probe was started more than once")))
            return
        }
        phase = .compiling
        guard let store = WKContentRuleListStore(url: cacheURL) else {
            finish(.failure(fail("WebKit could not create the isolated compiler store")))
            return
        }
        compileStore = store
        compileStartedAtNanoseconds = DispatchTime.now().uptimeNanoseconds
        store.compileContentRuleList(
            forIdentifier: expectedIdentifier,
            encodedContentRuleList: encodedRules
        ) { [weak self] list, error in
            let completedAt = DispatchTime.now().uptimeNanoseconds
            DispatchQueue.main.async {
                self?.didCompile(list: list, error: error, completedAt: completedAt)
            }
        }
    }

    func expire() {
        finish(.failure(fail("native WebKit blocker probe exceeded its 170-second deadline")))
    }

    private func didCompile(
        list: WKContentRuleList?,
        error: Error?,
        completedAt: UInt64
    ) {
        guard phase == .compiling else {
            duplicateOrOutOfOrderCallback = true
            return
        }
        guard let startedAt = compileStartedAtNanoseconds, completedAt >= startedAt else {
            finish(.failure(fail("native blocker compile timing was not monotonic")))
            return
        }
        coldCompileSeconds = TimeInterval(completedAt - startedAt) / 1_000_000_000
        guard error == nil, let list else {
            finish(.failure(fail("WebKit rejected the exact blocker artifact: \(describe(error))")))
            return
        }
        guard let actualIdentifier = list.identifier, actualIdentifier == expectedIdentifier else {
            finish(
                .failure(
                    fail(
                        "WebKit compiled identifier \(String(describing: list.identifier)) instead of \(expectedIdentifier)"
                    )
                )
            )
            return
        }

        phase = .lookingUp
        compileStore = nil
        guard let store = WKContentRuleListStore(url: cacheURL) else {
            finish(.failure(fail("WebKit could not reopen the isolated compiler store")))
            return
        }
        lookupStore = store
        lookupStartedAtNanoseconds = DispatchTime.now().uptimeNanoseconds
        store.lookUpContentRuleList(forIdentifier: expectedIdentifier) { [weak self] list, error in
            let completedAt = DispatchTime.now().uptimeNanoseconds
            DispatchQueue.main.async {
                self?.didLookUp(list: list, error: error, completedAt: completedAt)
            }
        }
    }

    private func didLookUp(
        list: WKContentRuleList?,
        error: Error?,
        completedAt: UInt64
    ) {
        guard phase == .lookingUp else {
            duplicateOrOutOfOrderCallback = true
            return
        }
        guard let startedAt = lookupStartedAtNanoseconds, completedAt >= startedAt else {
            finish(.failure(fail("native blocker cache-lookup timing was not monotonic")))
            return
        }
        cacheLookupSeconds = TimeInterval(completedAt - startedAt) / 1_000_000_000
        guard error == nil, let list else {
            finish(
                .failure(
                    fail("WebKit did not reload the compiled blocker artifact: \(describe(error))")
                )
            )
            return
        }
        guard let actualIdentifier = list.identifier, actualIdentifier == expectedIdentifier else {
            finish(
                .failure(
                    fail(
                        "WebKit reloaded identifier \(String(describing: list.identifier)) instead of \(expectedIdentifier)"
                    )
                )
            )
            return
        }
        guard
            let coldCompileSeconds,
            coldCompileSeconds <= productionColdCompileBudgetSeconds
        else {
            finish(
                .failure(
                    fail(
                        "cold native compile exceeded the production 120-second watchdog"
                    )
                )
            )
            return
        }
        finish(.success(()))
    }

    private func finish(_ result: Result<Void, ProbeFailure>) {
        guard phase != .terminal, terminalResult == nil else {
            duplicateOrOutOfOrderCallback = true
            return
        }
        phase = .terminal
        compileStore = nil
        lookupStore = nil
        terminalResult = result
    }

    private func describe(_ error: Error?) -> String {
        error.map(String.init(describing:)) ?? "missing native result and error"
    }
}

private func runNativeProbe(artifactPath: String) throws {
    let deadline = DispatchTime.now() + nativeDeadlineSeconds
    let artifact = try readExactRegularArtifact(at: artifactPath)
    guard let encodedRules = String(data: artifact, encoding: .utf8) else {
        throw fail("blocker artifact is not valid UTF-8")
    }
    var digestState = SHA256()
    digestState.update(data: artifactDigestDomain)
    var encodedFormatVersion = artifactFormatVersion.bigEndian
    withUnsafeBytes(of: &encodedFormatVersion) {
        digestState.update(data: Data($0))
    }
    digestState.update(data: artifact)
    let digest = digestState.finalize().map { String(format: "%02x", $0) }.joined()
    guard digest == expectedReleaseArtifactDigest else {
        throw fail(
            "blocker artifact digest \(digest) does not match the reviewed release golden"
        )
    }
    let expectedIdentifier = identifierPrefix + digest
    guard expectedIdentifier.utf8.count == identifierPrefix.utf8.count + 64 else {
        throw fail("blocker artifact identifier is not canonical")
    }

    let cacheURL = try isolatedCacheDirectory()
    defer {
        try? FileManager.default.removeItem(at: cacheURL)
    }
    let probe = NativeRuleStoreProbe(
        cacheURL: cacheURL,
        encodedRules: encodedRules,
        expectedIdentifier: expectedIdentifier
    )
    let deadlineTimer = DispatchSource.makeTimerSource(queue: .main)
    deadlineTimer.schedule(deadline: deadline)
    deadlineTimer.setEventHandler {
        probe.expire()
    }
    deadlineTimer.resume()
    defer {
        deadlineTimer.setEventHandler {}
        deadlineTimer.cancel()
    }
    probe.start()
    while probe.terminalResult == nil, DispatchTime.now().uptimeNanoseconds < deadline.uptimeNanoseconds {
        _ = RunLoop.main.run(
            mode: .default,
            before: Date(timeIntervalSinceNow: 0.05)
        )
    }
    if probe.terminalResult == nil {
        probe.expire()
    }

    let drainDeadline = DispatchTime.now() + duplicateCallbackDrainSeconds
    while DispatchTime.now().uptimeNanoseconds < drainDeadline.uptimeNanoseconds {
        _ = RunLoop.main.run(
            mode: .default,
            before: Date(timeIntervalSinceNow: 0.02)
        )
    }
    guard !probe.duplicateOrOutOfOrderCallback else {
        throw fail("WebKit delivered a duplicate or out-of-order blocker callback")
    }
    let compileMeasurement = probe.coldCompileSeconds.map {
        String(format: "%.6f", $0)
    } ?? "unsettled"
    let lookupMeasurement = probe.cacheLookupSeconds.map {
        String(format: "%.6f", $0)
    } ?? "unsettled"
    print(
        "native WKContentRuleListStore timings: cold_compile_seconds=\(compileMeasurement) "
            + "cache_lookup_seconds=\(lookupMeasurement)"
    )
    try probe.terminalResult?.get()
        ?? { throw fail("native WebKit blocker probe did not settle") }()
}

private func expectAdmissionFailure(_ path: String) throws {
    var rejected = false
    do {
        _ = try readExactRegularArtifact(at: path)
    } catch is ProbeFailure {
        rejected = true
    }
    guard rejected else {
        throw fail("unsafe blocker artifact fixture was accepted")
    }
}

private func runAdmissionSelfTest() throws {
    let root = try isolatedCacheDirectory()
    defer {
        try? FileManager.default.removeItem(at: root)
    }

    let regular = root.appendingPathComponent("regular.json")
    try Data("[]".utf8).write(to: regular, options: .withoutOverwriting)
    guard try readExactRegularArtifact(at: regular.path) == Data("[]".utf8) else {
        throw fail("regular blocker artifact self-test did not round trip")
    }

    let empty = root.appendingPathComponent("empty.json")
    try Data().write(to: empty, options: .withoutOverwriting)
    try expectAdmissionFailure(empty.path)

    let symbolic = root.appendingPathComponent("symbolic.json")
    try FileManager.default.createSymbolicLink(
        at: symbolic,
        withDestinationURL: regular
    )
    try expectAdmissionFailure(symbolic.path)

    let hardLink = root.appendingPathComponent("hard-link.json")
    guard link(regular.path, hardLink.path) == 0 else {
        throw posixFailure("cannot create hard-linked blocker fixture")
    }
    try expectAdmissionFailure(hardLink.path)
    try FileManager.default.removeItem(at: hardLink)

    let directory = root.appendingPathComponent("directory")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
    try expectAdmissionFailure(directory.path)

    let oversized = root.appendingPathComponent("oversized.json")
    let descriptor = open(oversized.path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, S_IRUSR | S_IWUSR)
    guard descriptor >= 0 else {
        throw posixFailure("cannot create oversized blocker fixture")
    }
    defer {
        _ = close(descriptor)
    }
    guard ftruncate(descriptor, maximumArtifactBytes + 1) == 0 else {
        throw posixFailure("cannot size oversized blocker fixture")
    }
    try expectAdmissionFailure(oversized.path)
}

@main
private struct MacOSBlockerSeedProbe {
    static func main() {
        do {
            let arguments = Array(CommandLine.arguments.dropFirst())
            if arguments == ["--self-test"] {
                try runAdmissionSelfTest()
                print("macOS blocker seed probe admission self-test passed")
                return
            }
            guard arguments.count == 1 else {
                throw fail("usage: probe_macos_blocker_seed ARTIFACT.json")
            }
            try runNativeProbe(artifactPath: arguments[0])
            print("exact bundled EasyList/EasyPrivacy artifact compiled and reloaded in WebKit")
        } catch {
            FileHandle.standardError.write(Data("macOS blocker seed probe failed: \(error)\n".utf8))
            Darwin.exit(1)
        }
    }
}
