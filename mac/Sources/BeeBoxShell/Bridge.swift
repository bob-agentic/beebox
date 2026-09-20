import AppKit
import WebKit

/// Serves the page's `window.__BEEBOX__` calls.
///
/// Every op replies exactly once. Replying twice traps; never replying leaves a
/// promise pending forever, which the user experiences as "I clicked and
/// nothing happened" — the exact failure this shell exists to eliminate.
@MainActor
final class Bridge: NSObject, WKScriptMessageHandlerWithReply {
    weak var window: NSWindow?
    /// The most recent mouse-down, kept because `performDrag(with:)` needs a
    /// real one and `NSApp.currentEvent` has usually moved on by the time a
    /// message has been round-tripped through JavaScript.
    var lastMouseDown: NSEvent?
    /// Set by the self-test to answer the folder chooser without showing it.
    var stubbedDirectory: (() -> String?)?

    nonisolated func userContentController(
        _ controller: WKUserContentController,
        didReceive message: WKScriptMessage,
        replyHandler: @escaping @MainActor @Sendable (Any?, String?) -> Void
    ) {
        // The body is read here because a WKScriptMessage cannot cross actors;
        // everything after that touches AppKit and so runs on the main actor.
        let body = message.body as? [String: Any]
        // A Task rather than `assumeIsolated`: the folder chooser replies from
        // a sheet callback long after this call has returned, and a reply sent
        // from outside a genuine main-actor context traps at runtime.
        Task { @MainActor in
            self.handle(body, reply: Self.once(replyHandler))
        }
    }

    private func handle(
        _ body: [String: Any]?,
        reply: @escaping @MainActor (Any?, String?) -> Void
    ) {
        guard let body, let op = body["op"] as? String else {
            reply(nil, "malformed bridge message")
            return
        }

        switch op {
        case "startDragging":
            if let event = lastMouseDown { window?.performDrag(with: event) }
            reply(nil, nil)
        case "close":
            window?.performClose(nil)
            reply(nil, nil)
        case "minimize":
            window?.miniaturize(nil)
            reply(nil, nil)
        case "toggleMaximize":
            window?.zoom(nil)
            reply(nil, nil)
        case "pickDirectory":
            pickDirectory(body, reply: reply)
        default:
            reply(nil, "unknown bridge op: \(op)")
        }
    }

    private func pickDirectory(
        _ body: [String: Any],
        reply: @escaping @MainActor (Any?, String?) -> Void
    ) {
        // `NSNull()` crosses into JavaScript as `null`, while Swift's `nil`
        // arrives as `undefined`. The page reads those differently: `null`
        // means "cancelled, do nothing", `undefined` means "there is no
        // chooser, ask them to type a path". Getting this backwards pops a
        // dialog at the moment the user said no.
        if let stub = stubbedDirectory {
            reply(stub() ?? NSNull(), nil)
            return
        }
        guard let window else {
            reply(NSNull(), nil)
            return
        }

        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "Open"
        panel.message = body["title"] as? String ?? "Open workspace"
        if let start = body["defaultPath"] as? String {
            panel.directoryURL = URL(fileURLWithPath: start)
        }

        panel.beginSheetModal(for: window) { response in
            // The sheet's completion runs on the main thread but outside any
            // actor context, so the hop back is explicit.
            let value: Any = response == .OK ? (panel.url?.path ?? "") : NSNull()
            Task { @MainActor in reply(value, nil) }
        }
    }

    /// Wraps a reply handler so a second call is dropped rather than fatal.
    private static func once(
        _ handler: @escaping @MainActor (Any?, String?) -> Void
    ) -> @MainActor (Any?, String?) -> Void {
        var called = false
        return { value, error in
            guard !called else { return }
            called = true
            handler(value, error)
        }
    }
}
