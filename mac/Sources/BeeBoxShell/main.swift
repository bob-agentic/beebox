import AppKit

// The menu is built before `run()`: AppKit reads `mainMenu` as it comes up, and
// a menu installed afterwards would not own its key equivalents for the first
// events. `.regular` is required too — an accessory app has no menu bar at all.
let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.mainMenu = MainMenu.build(target: delegate)
app.run()
