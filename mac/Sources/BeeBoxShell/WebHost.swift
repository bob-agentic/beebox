import AppKit
import WebKit

/// A web view that lets the menu keep its shortcuts.
@MainActor
final class ShellWebView: WKWebView {
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        // Decline exactly the chords the menu owns, so they carry on up to it.
        // Declining everything would break ⌘C and ⌘V in the terminal; declining
        // nothing would leave the menu at the mercy of WebKit's own table.
        if Commands.matches(event) { return false }
        return super.performKeyEquivalent(with: event)
    }

    /// Set by the page while the mouse is over a file link.
    var hoveredFile: String?

    /// Right-click on a file link offers to show it in Finder, above WebKit's
    /// own items.
    override func willOpenMenu(_ menu: NSMenu, with event: NSEvent) {
        super.willOpenMenu(menu, with: event)
        guard let path = hoveredFile else { return }
        let item = NSMenuItem(title: "Reveal in Finder", action: #selector(revealFile), keyEquivalent: "")
        item.target = self
        item.representedObject = path
        menu.insertItem(item, at: 0)
        menu.insertItem(.separator(), at: 1)
    }

    @objc private func revealFile(_ sender: NSMenuItem) {
        guard let path = sender.representedObject as? String else { return }
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }
}

/// WKWebView has no built-in `confirm()`/`alert()` UI: without a UI delegate
/// every confirm silently returns false, which would make ⌘W (close pane,
/// gated behind a confirm) dead on the desktop while working in every browser.
@MainActor
final class WebUI: NSObject, WKUIDelegate {
    func webView(
        _ webView: WKWebView,
        runJavaScriptAlertPanelWithMessage message: String,
        initiatedByFrame frame: WKFrameInfo
    ) async {
        let a = NSAlert()
        a.messageText = message
        a.addButton(withTitle: "OK")
        a.runModal()
    }

    func webView(
        _ webView: WKWebView,
        runJavaScriptConfirmPanelWithMessage message: String,
        initiatedByFrame frame: WKFrameInfo
    ) async -> Bool {
        let a = NSAlert()
        a.messageText = message
        a.addButton(withTitle: "OK")
        a.addButton(withTitle: "Cancel")
        return a.runModal() == .alertFirstButtonReturn
    }
}

@MainActor
enum WebHost {
    /// Builds the web view, with the bridge installed before any page script.
    static func make(bridge: Bridge, key: String, port: UInt16) -> ShellWebView {
        let controller = WKUserContentController()

        // Document-start, not `evaluateJavaScript`: the page decides whether it
        // is running in a shell once, at module scope, so anything injected
        // after load arrives too late and every shortcut silently dies.
        controller.addUserScript(
            WKUserScript(
                source: bootstrap(key: key, port: port),
                injectionTime: .atDocumentStart,
                forMainFrameOnly: true
            )
        )
        // `.page`, not an isolated world — the page's own scripts have to be
        // able to see `window.__BEEBOX__`.
        controller.addScriptMessageHandler(bridge, contentWorld: .page, name: "beebox")

        let config = WKWebViewConfiguration()
        config.userContentController = controller
        // Off unless asked for. The inspector is how interactions get verified
        // in the real webview rather than a browser, but leaving it on puts
        // "Inspect Element" and "Reload" in the right-click menu of a terminal
        // — which is the app admitting it is a web page in a window.
        //
        // BEEBOX_DEVTOOLS=1 brings it back for development.
        if ProcessInfo.processInfo.environment["BEEBOX_DEVTOOLS"] == "1" {
            config.preferences.setValue(true, forKey: "developerExtrasEnabled")
        }

        let web = ShellWebView(frame: .zero, configuration: config)
        bridge.web = web
        return web
    }

    /// The shell's entire JavaScript surface.
    private static func bootstrap(key: String, port: UInt16) -> String {
        """
        (() => {
          window.__BEEBOX_KEY__ = \(json(key));
          window.__BEEBOX_PORT__ = \(port);

          // WKScriptMessageHandlerWithReply makes postMessage return a promise,
          // so there is no callback table to keep.
          const post = (op, args) =>
            window.webkit.messageHandlers.beebox.postMessage({ op, ...(args || {}) });

          const listeners = new Set();
          window.__BEEBOX__ = {
            platform: 'macos',
            // Returns its own unsubscribe, so the caller can use it directly as
            // an effect teardown.
            onMenu(cb) { listeners.add(cb); return () => listeners.delete(cb); },
            // Called by the shell when a menu item fires. Not for page use.
            _menu(id) { for (const cb of [...listeners]) cb(id); },
            startDragging: () => post('startDragging'),
            close: () => post('close'),
            minimize: () => post('minimize'),
            toggleMaximize: () => post('toggleMaximize'),
            pickDirectory: (opts) => post('pickDirectory', opts),
            resolvePaths: (opts) => post('resolvePaths', opts),
            openFile: (opts) => post('openFile', opts),
            hoverFile: (opts) => post('hoverFile', opts),
          };
        })();
        """
    }

    static func json(_ value: String) -> String {
        let data = try? JSONSerialization.data(withJSONObject: [value], options: [])
        guard let data, let text = String(data: data, encoding: .utf8) else { return "\"\"" }
        return String(text.dropFirst().dropLast())
    }
}
