// swift-tools-version: 6.0
import PackageDescription

// No dependencies: AppKit and WebKit ship with the SDK, so this builds offline
// and without Xcode. Command Line Tools alone are enough.
//
// Tests are executables rather than a test target on purpose — this toolchain
// has neither XCTest nor swift-testing, so `swift test` cannot run. An exit
// code is a perfectly good assertion.
let package = Package(
    name: "BeeBoxShell",
    platforms: [.macOS(.v15)],
    targets: [
        .executableTarget(name: "BeeBoxShell", path: "Sources/BeeBoxShell"),
        .executableTarget(name: "beebox-contract", path: "Sources/beebox-contract"),
    ]
)
