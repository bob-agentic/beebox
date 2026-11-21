import AppKit

/// Paths printed in a terminal: which of them are real, and opening one.
enum FileLinks {
    /// The absolute path of each entry that names an existing file or
    /// directory, or nil.
    ///
    /// A relative path is tried against the shell's directory, then against its
    /// repository's root: an agent prints paths from the root even when the
    /// shell it runs in sits in a subdirectory.
    static func resolve(_ paths: [String], cwd: String) -> [String?] {
        let bases = [cwd, repoRoot(of: cwd)].compactMap { $0 }
        return paths.map { raw in
            let path = (raw as NSString).expandingTildeInPath
            let candidates = path.hasPrefix("/")
                ? [path]
                : bases.map { ($0 as NSString).appendingPathComponent(path) }
            return candidates.first(where: exists).map { ($0 as NSString).standardizingPath }
        }
    }

    /// Opens in the chosen editor — VS Code unless Zed is named — at the
    /// line, or with the file's default app when that editor is not
    /// installed. A directory opens in Finder: an editor would take it as a
    /// project and open a whole window.
    static func open(_ path: String, line: Int?, col: Int?, editor: String?) {
        guard exists(path) else { return }
        let workspace = NSWorkspace.shared
        if !isFile(path) {
            let dir = URL(fileURLWithPath: path, isDirectory: true)
            // An `.app` is a directory too, and opening one launches it.
            if workspace.isFilePackage(atPath: path) {
                workspace.activateFileViewerSelecting([dir])
            } else {
                workspace.open(dir)
            }
            return
        }
        var url = URLComponents()
        // Both take `<scheme>://file/<path>:<line>:<col>`.
        url.scheme = editor == "zed" ? "zed" : "vscode"
        url.host = "file"
        url.path = path + (line.map { ":\($0)" + (col.map { ":\($0)" } ?? "") } ?? "")
        if let url = url.url, workspace.urlForApplication(toOpen: url) != nil {
            workspace.open(url)
            return
        }
        let file = URL(fileURLWithPath: path)
        // A script's default app is Terminal, which runs it. A click on text
        // some program printed must never do that, so those are only shown.
        if FileManager.default.isExecutableFile(atPath: path) {
            workspace.activateFileViewerSelecting([file])
        } else {
            workspace.open(file)
        }
    }

    private static func exists(_ path: String) -> Bool {
        FileManager.default.fileExists(atPath: path)
    }

    private static func isFile(_ path: String) -> Bool {
        var dir: ObjCBool = false
        return FileManager.default.fileExists(atPath: path, isDirectory: &dir) && !dir.boolValue
    }

    private static func repoRoot(of dir: String) -> String? {
        var dir = dir
        while dir.count > 1 {
            if FileManager.default.fileExists(atPath: dir + "/.git") { return dir }
            dir = (dir as NSString).deletingLastPathComponent
        }
        return nil
    }
}
