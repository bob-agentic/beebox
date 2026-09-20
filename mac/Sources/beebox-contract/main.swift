// Checks that the native menu and the page agree about what commands exist.
//
// The old version of this test only looked one way — every menu id had to exist
// in the page — and so it stayed green while `close_pane` sat in the page's
// table with no menu item to invoke it, leaving ⌘W dead in the shipped app.
// Comparing both directions is the whole point.

import Foundation

let root = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()   // beebox-contract
    .deletingLastPathComponent()   // Sources
    .deletingLastPathComponent()   // mac
    .deletingLastPathComponent()   // repo root

var failures: [String] = []

// MARK: - The two tables

let commandsFile = root.appendingPathComponent("mac/Sources/BeeBoxShell/Commands.swift")
let appFile = root.appendingPathComponent("ui/src/App.svelte")

guard let commandsSource = try? String(contentsOf: commandsFile, encoding: .utf8),
      let appSource = try? String(contentsOf: appFile, encoding: .utf8)
else {
    print("contract: could not read the command tables")
    exit(1)
}

/// Every `Command(id: "…"` in the Swift table.
func menuIDs(_ source: String) -> [String] {
    matches(in: source, pattern: #"Command\(id:\s*"([a-z_]+)""#)
}

/// The keys of the page's `COMMANDS` record, read from between its braces so
/// that unrelated object literals elsewhere in the file cannot leak in.
/// Returns nil if the table could not be located at all.
func pageIDs(_ source: String) -> [String]? {
    guard let start = source.range(of: "const COMMANDS: Record<string, () => void> = {") else {
        return nil
    }
    var depth = 0
    var body = ""
    for character in source[start.lowerBound...] {
        if character == "{" { depth += 1 }
        if character == "}" {
            depth -= 1
            if depth == 0 { break }
        }
        if depth >= 1 { body.append(character) }
    }
    // Top-level keys only: nested arrow bodies are indented further.
    return matches(in: body, pattern: #"(?m)^\s{4}([a-z_]+):"#)
}

func matches(in source: String, pattern: String) -> [String] {
    guard let regex = try? NSRegularExpression(pattern: pattern) else { return [] }
    let range = NSRange(source.startIndex..., in: source)
    return regex.matches(in: source, range: range).compactMap { match in
        Range(match.range(at: 1), in: source).map { String(source[$0]) }
    }
}

let menu = menuIDs(commandsSource)
guard let page = pageIDs(appSource) else {
    print("contract: could not find the COMMANDS table in App.svelte")
    exit(1)
}

for id in Set(menu).subtracting(page).sorted() {
    failures.append("menu item \(id) has no entry in the page's COMMANDS table")
}
for id in Set(page).subtracting(menu).sorted() {
    failures.append("COMMANDS has \(id) but no menu item invokes it — it is unreachable")
}

// MARK: - No two commands may claim the same chord

// The key alone is not the shortcut — ⌘D and ⇧⌘D are different chords — so the
// modifiers are part of the identity being compared.
func chords(_ source: String) -> [String] {
    let pattern = #"key:\s*"([^"]+)",\s*modifiers:\s*\[([^\]]*)\]"#
    guard let regex = try? NSRegularExpression(pattern: pattern) else { return [] }
    let range = NSRange(source.startIndex..., in: source)
    return regex.matches(in: source, range: range).compactMap { match in
        guard let key = Range(match.range(at: 1), in: source),
              let mods = Range(match.range(at: 2), in: source)
        else { return nil }
        let normalised = source[mods]
            .split(separator: ",")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .sorted()
            .joined(separator: "+")
        return "\(normalised) \(source[key])"
    }
}

var seen = Set<String>()
let allChords = chords(commandsSource)
for chord in allChords where !seen.insert(chord).inserted {
    failures.append("two commands share the shortcut \(chord)")
}

// MARK: - The old shell must be gone

let uiSources = FileManager.default
    .enumerator(at: root.appendingPathComponent("ui/src"), includingPropertiesForKeys: nil)?
    .compactMap { $0 as? URL }
    .filter { ["ts", "svelte", "css"].contains($0.pathExtension) } ?? []

for file in uiSources {
    guard let text = try? String(contentsOf: file, encoding: .utf8) else { continue }
    if text.range(of: "__TAURI__|@tauri-apps|data-tauri", options: .regularExpression) != nil {
        let name = file.path.replacingOccurrences(of: root.path + "/", with: "")
        failures.append("\(name) still refers to the old Tauri shell")
    }
}

// MARK: - Result

if failures.isEmpty {
    let chordList = Set(allChords).count
    print("contract: \(menu.count) commands, menu ↔ COMMANDS agree, \(chordList) distinct shortcuts  OK")
    exit(0)
}
print("contract failed:")
for failure in failures { print("  FAIL  \(failure)") }
exit(1)
