import AppKit

/// Paths printed in a terminal: which of them are real files, and opening one.
enum FileLinks {
    /// The absolute path of each entry that names an existing file, or nil.
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
            return candidates.first(where: isFile).map { ($0 as NSString).standardizingPath }
        }
    }

    /// Opens in VS Code at the line, or with the file's default app when VS
    /// Code is not installed.
    static func open(_ path: String, line: Int?, col: Int?) {
        guard isFile(path) else { return }
        let workspace = NSWorkspace.shared
        var url = URLComponents()
        url.scheme = "vscode"
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
