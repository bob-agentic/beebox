import AppKit

/// The application menu, written out by hand.
///
/// This is also why the shortcuts work at all. AppKit only creates stock File
/// and Window menus if something asks it to, and nothing here does — so there
/// is no built-in ⌘N or ⌘T to lose the race against. The previous shell built
/// such a menu internally, and macOS consumed those chords before the web view
/// ever saw a key.
@MainActor
enum MainMenu {
    static func build(target: AnyObject) -> NSMenu {
        let root = NSMenu()
        root.addItem(appMenu())
        root.addItem(shellMenu(target: target))
        root.addItem(editMenu())
        root.addItem(viewMenu())
        return root
    }

    private static func submenu(_ title: String, _ items: [NSMenuItem]) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        let menu = NSMenu(title: title)
        for child in items { menu.addItem(child) }
        item.submenu = menu
        return item
    }

    /// The first submenu is the application menu; macOS titles it from the
    /// bundle name regardless of what is set here.
    private static func appMenu() -> NSMenuItem {
        submenu("BeeBox", [
            NSMenuItem(
                title: "About BeeBox",
                action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)),
                keyEquivalent: ""
            ),
            .separator(),
            NSMenuItem(
                title: "Hide BeeBox",
                action: #selector(NSApplication.hide(_:)),
                keyEquivalent: "h"
            ),
            {
                let item = NSMenuItem(
                    title: "Hide Others",
                    action: #selector(NSApplication.hideOtherApplications(_:)),
                    keyEquivalent: "h"
                )
                item.keyEquivalentModifierMask = [.command, .option]
                return item
            }(),
            .separator(),
            NSMenuItem(
                title: "Quit BeeBox",
                action: #selector(NSApplication.terminate(_:)),
                keyEquivalent: "q"
            ),
        ])
    }

    private static func shellMenu(target: AnyObject) -> NSMenuItem {
        submenu("Shell", Commands.all.map { command in
            let item = NSMenuItem(
                title: command.title,
                action: #selector(AppDelegate.runCommand(_:)),
                keyEquivalent: command.key
            )
            item.keyEquivalentModifierMask = command.modifiers
            // An explicit target: left to the responder chain, these are
            // disabled whenever the web view is first responder.
            item.target = target
            item.representedObject = command.id
            return item
        })
    }

    /// Copy and paste are the reason this exists — the terminal needs them, and
    /// leaving `target` nil is what lets them reach the web view.
    private static func editMenu() -> NSMenuItem {
        submenu("Edit", [
            NSMenuItem(title: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c"),
            NSMenuItem(title: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v"),
            NSMenuItem(
                title: "Select All",
                action: #selector(NSText.selectAll(_:)),
                keyEquivalent: "a"
            ),
        ])
    }

    private static func viewMenu() -> NSMenuItem {
        submenu("View", [
            {
                let item = NSMenuItem(
                    title: "Toggle Developer Tools",
                    action: #selector(AppDelegate.toggleDevTools(_:)),
                    keyEquivalent: "i"
                )
                item.keyEquivalentModifierMask = [.command, .option]
                return item
            }(),
        ])
    }
}
