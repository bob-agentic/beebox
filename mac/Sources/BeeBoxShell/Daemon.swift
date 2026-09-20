import Foundation

/// The `beebox-core` process this window talks to.
///
/// The shell owns the daemon's lifetime: quitting takes the terminals with it,
/// so there is never an orphaned server holding the port.
final class Daemon {
    enum StartError: Error, CustomStringConvertible {
        case binaryMissing(String)
        case noKey(log: String)
        case died(status: Int32, log: String)

        var description: String {
            switch self {
            case .binaryMissing(let path):
                return "beebox-core not found at \(path)"
            case .noKey(let log):
                return "daemon started but never printed an owner key.\n\n\(log)"
            case .died(let status, let log):
                return "daemon exited with status \(status).\n\n\(log)"
            }
        }
    }

    let port: UInt16
    let ownerKey: String
    private let process: Process

    private init(process: Process, port: UInt16, ownerKey: String) {
        self.process = process
        self.port = port
        self.ownerKey = ownerKey
    }

    /// Starts a daemon and waits for it to announce its owner key.
    ///
    /// Returning only once the key has arrived means the window cannot race the
    /// server: by the time there is a page to load, the port is accepting.
    static func start(
        binary: URL,
        home: URL,
        port: UInt16,
        bindHost: String,
        timeout: TimeInterval = 15
    ) throws -> Daemon {
        guard FileManager.default.isExecutableFile(atPath: binary.path) else {
            throw StartError.binaryMissing(binary.path)
        }

        let process = Process()
        process.executableURL = binary
        process.arguments = [
            "--listen", "\(bindHost):\(port)",
            "--home", home.path,
            // Without this the daemon outlives a crash of this process, keeping
            // the port and the terminals. A signal handler would not help: the
            // case that needs covering is SIGKILL, where nothing of ours runs.
            "--exit-with-parent",
        ]

        var env = ProcessInfo.processInfo.environment
        // The key is parsed out of the log line, and ANSI colour codes make that
        // needlessly fragile.
        env["NO_COLOR"] = "1"
        process.environment = env

        let pipe = Pipe()
        process.standardOutput = pipe
        // Merged, so a failure report carries the whole story rather than half.
        process.standardError = pipe

        try process.run()

        // Read the pipe directly rather than through `readabilityHandler`.
        // That callback is delivered by a run loop, and this is called before
        // the app has started one — waiting on a semaphore here would block the
        // main thread against a notification that can never arrive.
        let state = LogBuffer()
        let handle = pipe.fileHandleForReading
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            let data = handle.availableData    // blocks until there is output
            if data.isEmpty { break }          // EOF: the daemon has exited
            // Matched against the accumulated text, not this chunk: the key can
            // be split across a read boundary.
            if state.append(String(decoding: data, as: UTF8.self)) { break }
        }

        guard let key = state.key else {
            let status = process.isRunning ? 0 : process.terminationStatus
            process.terminate()
            throw status == 0
                ? StartError.noKey(log: state.text)
                : StartError.died(status: status, log: state.text)
        }

        // Past this point the daemon's output is only wanted for diagnostics,
        // and nothing is reading it — so drain it on a background queue rather
        // than let a full pipe buffer wedge the daemon mid-write.
        handle.readabilityHandler = { handle in
            let chunk = handle.availableData
            _ = state.append(String(decoding: chunk, as: UTF8.self))
            // Passed through as well: when something goes wrong the daemon's
            // log is the only account of it, and swallowing it leaves nothing
            // to debug from.
            FileHandle.standardError.write(chunk)
        }

        return Daemon(process: process, port: port, ownerKey: key)
    }

    /// Asks the daemon to stop, then insists.
    func stop() {
        guard process.isRunning else { return }
        process.terminate()
        // It exits cleanly on SIGTERM; this only covers a wedged one.
        let deadline = Date().addingTimeInterval(2)
        while process.isRunning && Date() < deadline {
            usleep(50_000)
        }
        if process.isRunning {
            kill(process.processIdentifier, SIGKILL)
        }
    }
}

/// Accumulates the daemon's output and notices the owner key going past.
private final class LogBuffer: @unchecked Sendable {
    private let lock = NSLock()
    private var buffer = ""
    private var found: String?

    var text: String { lock.withLock { buffer } }
    var key: String? { lock.withLock { found } }

    /// Returns true the first time the key appears.
    func append(_ chunk: String) -> Bool {
        lock.withLock {
            buffer += chunk
            guard found == nil,
                  let range = buffer.range(of: "key=[a-f0-9]{32}", options: .regularExpression)
            else { return false }
            found = String(buffer[range].dropFirst("key=".count))
            return true
        }
    }
}
