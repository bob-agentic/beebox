import AppKit

/// One entry per thing the menu can ask the page to do.
struct Command {
    /// The key in the page's `COMMANDS` table. The menu only names an intent;
    /// the behaviour lives in the page, so both ends must agree on this string.
    let id: String
    let title: String
    /// Lowercase, always. An uppercase letter makes AppKit add an implicit ⇧,
    /// which then displays as ⇧⌘D while quietly requiring a different chord.
    let key: String
    let modifiers: NSEvent.ModifierFlags
}

/// The shell's entire command surface.
///
/// The menu, the self-test and the contract check all read this one table, so a
/// command cannot appear in the menu without existing here — which is what
/// stops the menu and the page drifting apart. `close_pane` is the cautionary
/// tale: it was in the page's table but missing from the old Tauri menu, and
/// because the desktop build also disables the in-page keyboard handler, ⌘W was
/// unreachable while every test stayed green.
enum Commands {
    static let all: [Command] = [
        Command(id: "new_workspace", title: "New Workspace", key: "n", modifiers: [.command]),
        Command(id: "new_tab", title: "New Tab", key: "t", modifiers: [.command]),
        Command(id: "split_v", title: "Split Right", key: "d", modifiers: [.command]),
        Command(id: "split_h", title: "Split Down", key: "d", modifiers: [.command, .shift]),
        Command(id: "close_pane", title: "Close Pane", key: "w", modifiers: [.command]),
        Command(id: "prev_tab", title: "Previous Tab", key: "[", modifiers: [.command, .shift]),
        Command(id: "next_tab", title: "Next Tab", key: "]", modifiers: [.command, .shift]),
    ]

    /// Whether an event is one the menu owns. The web view uses this to decline
    /// exactly these chords and let them reach the menu, while keeping ⌘C and
    /// ⌘V for the terminal.
    nonisolated static func matches(_ event: NSEvent) -> Bool {
        let mods = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
            .intersection([.command, .shift, .option, .control])
        guard let chars = event.charactersIgnoringModifiers?.lowercased() else { return false }
        return all.contains { $0.key == chars && $0.modifiers == mods }
    }
}
