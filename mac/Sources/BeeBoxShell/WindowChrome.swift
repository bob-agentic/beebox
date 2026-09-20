import AppKit

@MainActor
enum WindowChrome {
    /// A window whose content reaches into the title bar, so the page can draw
    /// its tabs alongside the traffic lights.
    ///
    /// The traffic lights themselves are left exactly where macOS puts them:
    /// x∈[7,61], centred 14px from the top. Moving them is undone on every
    /// resize, and a title bar accessory does not re-centre them either. The
    /// page reserves that space instead.
    static func makeWindow(content: NSView) -> NSWindow {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1400, height: 880),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = "BeeBox"
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        // The page decides what is draggable; leaving this on would let the
        // terminal body move the window too.
        window.isMovableByWindowBackground = false
        window.minSize = NSSize(width: 720, height: 480)
        window.setFrameAutosaveName("BeeBoxMain")
        window.tabbingMode = .disallowed

        content.frame = window.contentLayoutRect
        content.autoresizingMask = [.width, .height]
        window.contentView = content
        window.center()
        return window
    }
}
