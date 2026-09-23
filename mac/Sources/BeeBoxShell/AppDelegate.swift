import AppKit
import WebKit

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, WKNavigationDelegate {
    private var window: NSWindow!
    private var web: ShellWebView!
    private let bridge = Bridge()
    // Held strongly: `uiDelegate` is weak, and without this confirm() would
    // silently revert to returning false once the delegate deallocates.
    private let webUI = WebUI()
    private var daemon: Daemon?
    private var mouseMonitor: Any?
    private var selfTest: SelfTest?
    private var screenshotPath: String?

    /// Where the daemon listens. Every interface by default — but the daemon
    /// refuses non-loopback clients until the owner flips the in-app
    /// "web server" toggle, so nothing is exposed until asked. Binding wide
    /// up front is what lets sharing start without a daemon restart.
    private let bindHost = ProcessInfo.processInfo.environment["BEEBOX_BIND"] ?? "0.0.0.0"

    func applicationDidFinishLaunching(_ notification: Notification) {
        let isSelfTest = CommandLine.arguments.contains("--self-test")
        screenshotPath = Self.argumentValue(after: "--screenshot")
        let isIsolatedRun = isSelfTest || screenshotPath != nil

        do {
            daemon = try Daemon.start(
                binary: Self.coreBinary(),
                home: Self.home(isSelfTest: isIsolatedRun),
                // Automated runs must coexist with the real app. Always give
                // them an isolated port instead of probing the app's default.
                port: isIsolatedRun ? Self.ephemeralPort() : Self.choosePort(preferred: 17788),
                bindHost: bindHost
            )
        } catch {
            // Loudly: a shell that opens a window onto a daemon that never
            // started is the worst of both worlds.
            fail("BeeBox could not start its daemon.", detail: "\(error)")
            return
        }
        guard let daemon else { return }

        web = WebHost.make(bridge: bridge, key: daemon.ownerKey, port: daemon.port)
        web.navigationDelegate = self
        web.uiDelegate = webUI
        window = WindowChrome.makeWindow(content: web)
        bridge.window = window

        // `performDrag(with:)` needs a genuine mouse-down, and by the time the
        // page has asked for a drag the current event is something else.
        mouseMonitor = NSEvent.addLocalMonitorForEvents(matching: .leftMouseDown) {
            [weak self] event in
            self?.bridge.lastMouseDown = event
            return event
        }

        // `localhost`, not 127.0.0.1: the ATS exception is
        // NSAllowsLocalNetworking, which covers the hostname but not the
        // literal address. The page's socket URL has to match, or it is
        // cross-origin.
        let url = URL(string: "http://localhost:\(daemon.port)/?key=\(daemon.ownerKey)")!
        web.load(URLRequest(url: url))

        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        if isSelfTest {
            selfTest = SelfTest(web: web, bridge: bridge) { [weak self] in self?.daemon?.stop() }
        }
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        selfTest?.begin()
        guard let screenshotPath else { return }
        // Navigation completes before the socket has delivered its first tree
        // and before xterm has had a frame to paint. Snapshot the composited
        // web view after that short settling period.
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) { [weak self] in
            self?.takeSnapshot(to: screenshotPath)
        }
    }

    func webView(
        _ webView: WKWebView,
        didFailProvisionalNavigation navigation: WKNavigation!,
        withError error: Error
    ) {
        fail("BeeBox could not load its interface.", detail: error.localizedDescription)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func applicationWillTerminate(_ notification: Notification) {
        if let mouseMonitor { NSEvent.removeMonitor(mouseMonitor) }
        daemon?.stop()
    }

    // MARK: - Menu

    /// The menu names an intent; the page owns the behaviour.
    @objc func runCommand(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        web.evaluateJavaScript("window.__BEEBOX__._menu(\(WebHost.json(id)))")
    }

    @objc func toggleDevTools(_ sender: Any?) {
        // Not exposed in the public API, but the inspector is the only way to
        // debug the page in the real webview.
        //
        // Does nothing unless the app was launched with BEEBOX_DEVTOOLS=1:
        // the inspector is off by default so that "Inspect Element" stays out
        // of the right-click menu. See WebHost.
        if web.responds(to: Selector(("_inspector"))) {
            web.perform(Selector(("_showInspector")))
        }
    }

    // MARK: - Placement

    private static func coreBinary() -> URL {
        if let override = ProcessInfo.processInfo.environment["BEEBOX_CORE"] {
            return URL(fileURLWithPath: override)
        }
        // Bundled: right next to this executable. Otherwise a dev build.
        let sibling = Bundle.main.bundleURL
            .appendingPathComponent("Contents/MacOS/beebox-core")
        if FileManager.default.isExecutableFile(atPath: sibling.path) { return sibling }
        return URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()      // BeeBoxShell
            .deletingLastPathComponent()      // Sources
            .deletingLastPathComponent()      // mac
            .deletingLastPathComponent()      // repo root
            .appendingPathComponent("core/target/release/beebox-core")
    }

    private static func home(isSelfTest: Bool) -> URL {
        let base = FileManager.default.homeDirectoryForCurrentUser
        // The self-test must not disturb real workspaces, and must start from a
        // known-empty tree for its counts to mean anything.
        guard isSelfTest else {
            let dir = Bundle.main.object(forInfoDictionaryKey: "BeeBoxHome") as? String
            return base.appendingPathComponent(dir ?? ".beebox")
        }
        let scratch = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("beebox-self-test-\(ProcessInfo.processInfo.processIdentifier)")
        try? FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        return scratch
    }

    private static func argumentValue(after flag: String) -> String? {
        guard let i = CommandLine.arguments.firstIndex(of: flag),
              CommandLine.arguments.indices.contains(i + 1)
        else { return nil }
        return CommandLine.arguments[i + 1]
    }

    private func takeSnapshot(to path: String) {
        let config = WKSnapshotConfiguration()
        config.rect = web.bounds
        web.takeSnapshot(with: config) { [weak self] image, error in
            guard let self else { return }
            defer {
                self.daemon?.stop()
                NSApp.terminate(nil)
            }
            do {
                if let error { throw error }
                guard let tiff = image?.tiffRepresentation,
                      let bitmap = NSBitmapImageRep(data: tiff),
                      let png = bitmap.representation(using: .png, properties: [:])
                else {
                    throw CocoaError(.fileWriteUnknown)
                }
                let url = URL(fileURLWithPath: path)
                try FileManager.default.createDirectory(
                    at: url.deletingLastPathComponent(),
                    withIntermediateDirectories: true
                )
                try png.write(to: url, options: .atomic)
                print("snapshot: \(url.path)")
            } catch {
                FileHandle.standardError.write(Data("snapshot failed: \(error)\n".utf8))
            }
        }
    }

    /// The preferred port if it is free, otherwise one the OS picks.
    ///
    /// An already-running BeeBox is not reused: its owner key was printed to
    /// its own stdout, which this process cannot read, so its port would be
    /// reachable but unusable.
    private static func choosePort(preferred: UInt16) -> UInt16 {
        if isFree(preferred) { return preferred }
        return ephemeralPort()
    }

    private static func isFree(_ port: UInt16) -> Bool {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { return false }
        defer { Darwin.close(fd) }
        var yes: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &yes, socklen_t(MemoryLayout<Int32>.size))
        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = port.bigEndian
        addr.sin_addr.s_addr = INADDR_ANY
        let bound = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        return bound == 0
    }

    private static func ephemeralPort() -> UInt16 {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        defer { Darwin.close(fd) }
        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = 0
        addr.sin_addr.s_addr = INADDR_ANY
        _ = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        var out = sockaddr_in()
        var len = socklen_t(MemoryLayout<sockaddr_in>.size)
        _ = withUnsafeMutablePointer(to: &out) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                getsockname(fd, $0, &len)
            }
        }
        return UInt16(bigEndian: out.sin_port)
    }

    private func fail(_ message: String, detail: String) {
        if CommandLine.arguments.contains("--self-test") {
            FileHandle.standardError.write(Data("\(message)\n\(detail)\n".utf8))
            exit(1)
        }
        let alert = NSAlert()
        alert.messageText = message
        alert.informativeText = detail
        alert.alertStyle = .critical
        alert.runModal()
        NSApp.terminate(nil)
    }
}
