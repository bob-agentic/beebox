import AppKit
import WebKit

/// Drives the real app through its real menu and checks what actually happened.
///
/// This exists because the browser end-to-end suite cannot see any of it. There
/// is no menu bar in a headless Chromium, so a shortcut can be completely dead
/// in the shipped app while every one of those tests passes — which is exactly
/// how ⌘N came to do nothing at all.
///
/// Each case synthesises a key event indistinguishable from a real one and
/// hands it to the live main menu, then waits for the tree to change. That
/// covers the whole chain: the key equivalent table, whether the item is
/// enabled, its target and action, the command id it carries, the JavaScript
/// call, the page's listener, its COMMANDS table, and the round trip through
/// the daemon to a real PTY.
@MainActor
final class SelfTest {
    private let web: WKWebView
    private let bridge: Bridge
    /// Called before exiting, so the daemon stops with us. Waiting for
    /// `--exit-with-parent` to notice would leave the port held for seconds —
    /// long enough for the next run to fail to bind.
    private let cleanup: () -> Void
    private var failures: [String] = []
    private var started = false

    init(web: WKWebView, bridge: Bridge, cleanup: @escaping () -> Void) {
        self.web = web
        self.bridge = bridge
        self.cleanup = cleanup
    }

    func begin() {
        guard !started else { return }
        started = true
        Task { await self.run() }
    }

    private func run() async {
        // Everything below assumes a connected socket and a first pane.
        guard await waitFor("document.querySelectorAll('.sheet.on .pane').length", equals: 1) else {
            finish(fatal: "the first pane never appeared")
            return
        }

        // If this is false the bridge was installed too late and nothing else
        // in this file can be trusted.
        await check("bridge is visible to the page", "window.__BEEBOX__ !== undefined", is: true)
        await check("page knows it is in a shell", "document.body.classList.contains('web')", is: false)

        // xterm's WebGL renderer silently produces an empty canvas in some
        // webviews: the pane has the right size, the text is in the buffer, and
        // nothing is drawn. Check the context exists before trusting the eye.
        await check(
            "the webview can create a WebGL context",
            """
            (() => {
              const c = document.createElement('canvas');
              return !!(c.getContext('webgl2') || c.getContext('webgl'));
            })()
            """,
            is: true
        )

        await expectNewTerminal(
            commandID: "new_tab",
            counting: ".tab",
            label: "new tab",
            marker: "NEW_TAB_ALIVE"
        )
        await expectNewTerminal(
            commandID: "new_workspace",
            counting: ".ws",
            label: "new workspace",
            marker: "NEW_WORKSPACE_ALIVE"
        )
        await expect("split_v", counting: ".sheet.on .pane", by: +1)
        await expect("split_h", counting: ".sheet.on .pane", by: +1)
        // The one the old shell could not do at all: ⌘W was absent from its
        // menu, and the in-page keyboard path is disabled on desktop.
        // close_pane asks for confirmation; a native NSAlert would hang an
        // unattended run, so answer it in the page before it can appear.
        _ = try? await web.evaluateJavaScript("window.confirm = () => true; 0")
        await expect("close_pane", counting: ".sheet.on .pane", by: -1)

        // Cycling needs somewhere to cycle to, and the workspace opened above
        // starts with a single tab.
        await expect("new_tab", counting: ".tab", by: +1)
        await expectTabChange("prev_tab")
        await expectTabChange("next_tab")

        // The user's path: several workspaces, then switching back. A pane that
        // was alive when it was last on screen must still be showing its
        // scrollback when you return to it.
        await checkTerminalSurvivesSwitching()

        await checkPickDirectory()
        // Counting panes proves a box was drawn, not that anything is running
        // in it. A shell that never printed its prompt looks identical to a
        // working one by every other assertion here.
        await checkTerminalIsLive()
        finish(fatal: nil)
    }

    /// Reproduces the user's exact path through the native menu, then uses the
    /// new terminal before a split, switch or resize can hide a paint bug.
    private func expectNewTerminal(
        commandID: String,
        counting selector: String,
        label: String,
        marker: String
    ) async {
        guard let command = Commands.all.first(where: { $0.id == commandID }) else {
            failures.append("\(commandID): not in the command table")
            return
        }
        let count = "document.querySelectorAll('\(selector)').length"
        guard let before = await number(count) else {
            failures.append("\(commandID): could not count \(selector)")
            return
        }
        guard NSApp.mainMenu?.performKeyEquivalent(with: event(for: command)) == true else {
            failures.append("\(commandID): the menu did not claim \(describe(command))")
            return
        }
        guard await waitFor(count, equals: before + 1) else {
            failures.append("\(commandID): \(label) did not appear")
            return
        }
        report("\(commandID)  \(describe(command))  \(selector) \(before) → \(before + 1)")

        let text = "document.querySelector('.sheet.on .pane .term')?.__serialize?.() ?? ''"
        guard await waitUntil(timeout: 8, { (await self.string(text) ?? "").count > 1 }) else {
            failures.append("\(label) terminal never produced its first prompt")
            return
        }
        await typeIntoTerminal("echo \(marker)\r")
        guard await waitUntil(timeout: 8, {
            (await self.string(text) ?? "").contains(marker)
        }) else {
            failures.append("\(label) terminal did not accept input")
            return
        }
        report("\(label) terminal accepts input before any resize")
        await checkTerminalIsDrawn(label: "\(label) terminal")
    }

    /// Switching away from a workspace and back must not blank its terminals.
    private func checkTerminalSurvivesSwitching() async {
        let text = "document.querySelector('.sheet.on .pane .term')?.__serialize?.() ?? ''"
        guard await waitUntil(timeout: 8, { (await self.string(text) ?? "").count > 1 }) else {
            failures.append("the terminal was blank before switching")
            return
        }
        await typeIntoTerminal("echo SWITCH_MARKER\r")
        guard await waitUntil(timeout: 8, {
            (await self.string(text) ?? "").contains("SWITCH_MARKER")
        }) else {
            failures.append("the terminal never showed SWITCH_MARKER")
            return
        }

        // Away, then back.
        _ = try? await web.callAsyncJavaScript(
            "document.querySelectorAll('.ws')[0]?.click(); return true;",
            contentWorld: .page
        )
        try? await Task.sleep(nanoseconds: 700_000_000)
        _ = try? await web.callAsyncJavaScript(
            "document.querySelectorAll('.ws')[1]?.click(); return true;",
            contentWorld: .page
        )

        if await waitUntil(timeout: 8, {
            (await self.string(text) ?? "").contains("SWITCH_MARKER")
        }) {
            report("a terminal keeps its scrollback across a workspace switch")
        } else {
            let seen = (await string(text) ?? "").suffix(80)
            failures.append("the terminal went blank after switching back. Saw: \(seen)")
        }
    }

    /// The visible terminal must contain the output of a command.
    private func checkTerminalIsLive() async {
        // WebGL renders to a canvas, so there is nothing in the DOM to read;
        // the page exposes xterm's own serializer for exactly this.
        let text = "document.querySelector('.sheet.on .pane .term')?.__serialize?.() ?? ''"

        guard await waitUntil(timeout: 8, { (await self.string(text) ?? "").count > 1 }) else {
            failures.append("the terminal is blank — no shell prompt was ever drawn")
            return
        }

        // And it has to accept input, not just show something from startup.
        _ = try? await web.callAsyncJavaScript(
            """
            const pane = document.querySelector('.sheet.on .pane .term');
            pane?.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
            pane?.querySelector('textarea')?.focus();
            return true;
            """,
            contentWorld: .page
        )
        await typeIntoTerminal("echo SELFTEST_ALIVE\r")

        if await waitUntil(timeout: 8, {
            (await self.string(text) ?? "").contains("SELFTEST_ALIVE")
        }) {
            report("the terminal runs a command and shows its output")
            await checkTerminalIsDrawn(label: "terminal")
        } else {
            let seen = (await string(text) ?? "").suffix(120)
            failures.append("the terminal did not echo a command back. Last saw: \(seen)")
        }
    }

    /// Having the right text in the buffer is not the same as it appearing on
    /// screen. Whichever renderer xterm picked has to leave something visible:
    /// rows of text in the DOM, or a canvas with lit pixels.
    private func checkTerminalIsDrawn(label: String) async {
        let drawn = await number(
            """
            (() => {
              const host = document.querySelector('.sheet.on .pane .term');
              if (!host) return -1;
              // DOM renderer: xterm writes one element per row.
              const rows = host.querySelector('.xterm-rows');
              if (rows) {
                const shown = rows.textContent?.replace(/\\s/g, '') ?? '';
                return shown.length;
              }
              // Canvas renderers use several layers. The first can be an empty
              // selection/link layer, so composite every canvas before looking
              // for glyph pixels.
              const layers = [...host.querySelectorAll('canvas')];
              if (!layers.length) return -2;
              const width = Math.max(...layers.map(c => c.width));
              const height = Math.min(160, Math.max(...layers.map(c => c.height)));
              const g = document.createElement('canvas').getContext('2d');
              g.canvas.width = width;
              g.canvas.height = height;
              try { for (const c of layers) g.drawImage(c, 0, 0); } catch { return -3; }
              const d = g.getImageData(0, 0, width, height).data;
              const r0 = d[0], g0 = d[1], b0 = d[2];
              let n = 0;
              for (let i = 0; i < d.length; i += 4) {
                if (Math.abs(d[i]-r0) + Math.abs(d[i+1]-g0) + Math.abs(d[i+2]-b0) > 30) n++;
              }
              return n;
            })()
            """
        ) ?? -4
        if drawn > 20 {
            report("the \(label) is actually drawn on screen (\(drawn))")
        } else {
            let detail = await string(
                """
                (() => {
                  const host = document.querySelector('.sheet.on .pane .term');
                  const rows = host?.querySelector('.xterm-rows');
                  const screen = host?.querySelector('.xterm-screen');
                  return JSON.stringify({
                    hostBox: host ? [host.clientWidth, host.clientHeight] : null,
                    screenBox: screen ? [screen.clientWidth, screen.clientHeight] : null,
                    rowCount: rows?.children.length ?? -1,
                    rowsText: (rows?.textContent ?? '').slice(0, 60),
                    renderer: host?.querySelector('canvas') ? 'canvas' : 'dom',
                    canvasCount: host?.querySelectorAll('canvas').length ?? 0,
                    buffer: (host?.__serialize?.() ?? '').replace(/\\u001b\\[[0-9;]*m/g, '').slice(0, 80),
                    rowsHTML: (rows?.children[0]?.outerHTML ?? '').slice(0, 120),
                  });
                })()
                """
            ) ?? "?"
            failures.append(
                "nothing is painted in the \(label) (\(drawn)). \(detail)"
            )
        }
    }

    private func typeIntoTerminal(_ text: String) async {
        // Straight into xterm's own handler: synthesising key events through
        // AppKit would depend on focus, which is what this is trying to test
        // around rather than through.
        _ = try? await web.callAsyncJavaScript(
            """
            const host = document.querySelector('.sheet.on .pane .term');
            host?.__type?.(text);
            return true;
            """,
            arguments: ["text": text],
            contentWorld: .page
        )
    }

    // MARK: - Cases

    /// Fires a command's real shortcut and waits for a count to move.
    private func expect(_ id: String, counting selector: String, by delta: Int) async {
        guard let command = Commands.all.first(where: { $0.id == id }) else {
            failures.append("\(id): not in the command table")
            return
        }
        let expression = "document.querySelectorAll('\(selector)').length"
        guard let before = await number(expression) else {
            failures.append("\(id): could not read \(selector)")
            return
        }

        guard NSApp.mainMenu?.performKeyEquivalent(with: event(for: command)) == true else {
            failures.append("\(id): the menu did not claim \(describe(command))")
            return
        }

        let target = before + delta
        if await waitFor(expression, equals: target) {
            report("\(id)  \(describe(command))  \(selector) \(before) → \(target)")
        } else {
            let now = await number(expression).map(String.init) ?? "?"
            failures.append(
                "\(id): expected \(selector) to go \(before) → \(target), got \(now)"
            )
        }
    }

    private func expectTabChange(_ id: String) async {
        guard let command = Commands.all.first(where: { $0.id == id }) else {
            failures.append("\(id): not in the command table")
            return
        }
        let expression = "document.querySelector('.tab.active .label')?.textContent ?? ''"
        let before = await string(expression) ?? ""

        guard NSApp.mainMenu?.performKeyEquivalent(with: event(for: command)) == true else {
            failures.append("\(id): the menu did not claim \(describe(command))")
            return
        }

        let changed = await waitUntil {
            (await self.string(expression) ?? before) != before
        }
        if changed {
            report("\(id)  \(describe(command))  active tab moved")
        } else {
            failures.append("\(id): the active tab did not change")
        }
    }

    /// The folder chooser's three outcomes, which the page reads differently.
    private func checkPickDirectory() async {
        bridge.stubbedDirectory = { "/tmp" }
        await check(
            "pickDirectory returns the chosen path",
            "await window.__BEEBOX__.pickDirectory({}) === '/tmp'",
            is: true
        )
        // Cancelling must be `null`, never `undefined`: the page treats
        // `undefined` as "there is no chooser" and falls back to asking the
        // user to type a path — a dialog appearing right after they cancelled.
        bridge.stubbedDirectory = { nil }
        await check(
            "cancelling returns null, not undefined",
            "await window.__BEEBOX__.pickDirectory({}) === null",
            is: true
        )
        bridge.stubbedDirectory = nil

    }

    // MARK: - Plumbing

    private func check(_ name: String, _ expression: String, is expected: Bool) async {
        let actual = await boolean("(async () => (\(expression)))()")
        if actual == expected {
            report(name)
        } else {
            failures.append("\(name): expected \(expected), got \(actual.map(String.init) ?? "nil")")
        }
    }

    /// A key event shaped exactly like the real chord, so the menu cannot tell
    /// the difference.
    private func event(for command: Command) -> NSEvent {
        NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: command.modifiers,
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: 0,
            context: nil,
            characters: command.key,
            charactersIgnoringModifiers: command.key,
            isARepeat: false,
            keyCode: 0
        )!
    }

    private func describe(_ command: Command) -> String {
        var text = ""
        if command.modifiers.contains(.control) { text += "⌃" }
        if command.modifiers.contains(.option) { text += "⌥" }
        if command.modifiers.contains(.shift) { text += "⇧" }
        if command.modifiers.contains(.command) { text += "⌘" }
        return text + command.key.uppercased()
    }

    private func waitFor(_ expression: String, equals value: Int) async -> Bool {
        await waitUntil { await self.number(expression) == value }
    }

    /// Polls, because every command's effect crosses the socket and comes back.
    private func waitUntil(
        timeout: TimeInterval = 10,
        _ condition: () async -> Bool
    ) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if await condition() { return true }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        return false
    }

    private func number(_ expression: String) async -> Int? {
        (try? await web.evaluateJavaScript(expression) as Any).flatMap { $0 as? Int }
    }

    private func string(_ expression: String) async -> String? {
        (try? await web.evaluateJavaScript(expression) as Any).flatMap { $0 as? String }
    }

    private func boolean(_ expression: String) async -> Bool? {
        (try? await web.callAsyncJavaScript(
            "return \(expression);",
            contentWorld: .page
        ) as Any).flatMap { $0 as? Bool }
    }

    private func report(_ line: String) {
        print("  ok    \(line)")
    }

    private func finish(fatal: String?) {
        cleanup()
        if let fatal { failures.append(fatal) }
        if failures.isEmpty {
            print("\nself-test: every shortcut reached the page and changed the tree")
            exit(0)
        }
        print("\nself-test failed:")
        for failure in failures { print("  FAIL  \(failure)") }
        exit(1)
    }
}
