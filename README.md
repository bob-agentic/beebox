<div align="center">

# 🐝 BeeBox

**A web terminal built for running many agent CLIs in parallel.**

`v0.2.6`

</div>

---

## TL;DR — why web-based, not tmux-based

BeeBox is a terminal whose surface is the **browser**, not a character grid.
That one choice is the whole difference:

- **Sharing that actually works, on any platform.** A session's endpoint is a
  **URL** — open it on a phone, an iPad, Windows, someone else's browser, no
  client to install. Cross-platform sharing is free, not a feature you bolt on.
  A tmux-based tool ties sharing to environments that can attach to tmux.
- **The whole web platform is the extension surface.** A pane is DOM/canvas, so
  a plugin can be a diff view, an image preview, a rich panel — anything the web
  can render. A tmux-based tool can only ever extend a character grid, because
  that is all it draws.
- **Multimedia is native, not fought.** Text selection, **pasting images**,
  `@path` file uploads — these are built-in browser capabilities. In a character
  terminal the same needs are a running battle (copy-mode, OSC 52, and hacks).

Tools like `JackTPatterson/herd` put a native shell over a tmux/Herdr runtime —
familiar on the surface, but the core is still a character grid, and that grid
caps sharing, extensibility, and multimedia. BeeBox's core is the browser.

---

**It is a terminal first.** Raw PTY bytes go to the browser untouched — the
server does no VT parsing and no output summarization. On top of an ordinary
terminal it adds two main things: a **status dot per pane**, driven by agent
hooks, and the ability to **share any level of a session over a URL**.

---

## Install

Download from [Releases](https://github.com/bob-agentic/beebox/releases/latest).

### macOS (Apple Silicon only)

Unzip `BeeBox-vX.Y.Z-macos.zip` and drag **BeeBox.app** into Applications. The
app is not notarized, so macOS will refuse to open it ("damaged" or "cannot be
verified"). Clear the quarantine flag once:

```bash
xattr -dr com.apple.quarantine /Applications/BeeBox.app
```

Everything is inside the app — interface, fonts and daemon. The daemon is its
child process and quitting the app takes every terminal with it; nothing is
left running behind.

### Android

Install `BeeBox-vX.Y.Z-android.apk` (Android 10+, arm64/armv7). You will need to
allow installing from your browser or file manager. The app holds no terminals
of its own: it connects to a BeeBox on your computer. Point the system camera
at a share code and the session opens in the app — or scan or paste a link from
inside it.

### Headless daemon (a server, or any machine without the app)

```bash
cd ui && pnpm install && pnpm build        # the daemon embeds the interface
cd ../core && cargo build --release
./target/release/beebox-core
```

On startup it prints the **full URL, owner key included**, for every reachable
address on the machine:

```
open:  http://localhost:17788/?key=e824e393...
       http://192.168.x.xxx:17788/?key=e824e393...
```

**You must use the URL with the key.** The default bind is `0.0.0.0`; without a
key, anyone on the same network who opens that port gets your whole machine's
terminals — so "no token" means **zero access**, not full access.

The key is regenerated on every start and never persisted: leak it, restart, and
it's dead. To give someone access, send a **share link**, not this URL.

> ### On actually letting someone connect — the last mile is yours
>
> **BeeBox only does the "sharing" part itself**: it mints token-bearing share
> links, enforces four scopes (All / Workspace / Tab / Pane), binds each link to
> the first device that opens it, and lets you see and disconnect those devices.
> **How you actually get that port in front of another person — a tunnel, frp,
> [Tailscale](https://tailscale.com),
> [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/),
> a reverse proxy, or anything else — BeeBox does not decide for you, and ships
> no tunneling of its own.**
>
> You're an engineer; you know how to expose a local port. Pick whatever suits
> you — **the last mile is yours to complete.** What BeeBox owns is what happens
> once someone can reach the port: how access is scoped, and how a share is
> revoked.

| Flag | Default | Meaning |
|---|---|---|
| `--listen` | `0.0.0.0:17788` | Bind address. How it's exposed is up to you; no restrictions imposed. |
| `--home` | `~/.beebox` | State directory (the layout database). |
| `--scrollback` | `200000` | Scrollback lines per pane, kept by the daemon. |
| `--ui` | (embedded) | Serve the frontend from disk. For development. |
| `--exit-with-parent` | off | Exit when the parent process does (the desktop app uses this to own the daemon's lifetime). |

---

## What it does

- **A real terminal.** PTY pool, raw byte passthrough, 200k lines of scrollback
  kept by the daemon, correct CJK and emoji widths, Chinese IME that works.
- **Workspaces × tabs × splits.** A workspace is a project directory; each has
  tabs, each tab a split tree (capped at two levels). Everything is saved and
  restored on restart, and terminals in the background keep running.
- **Agent status.** Claude Code and Codex report through hooks: a status dot per
  pane (rolled up to its tab and workspace), the tool being run, a summary when
  it finishes, automatic tab titles, and sessions resumed after a restart.
  Toggle them under Settings → Agents.
- **Sharing by link or QR code.** Four scopes — All / Workspace / Tab / Pane. A
  link belongs to the first device that opens it; every paired device is listed
  under Connections until you disconnect it. A scanned code lets the phone set
  the terminal's width.
- **Open files from the terminal (macOS).** ⌘-click a path an agent printed to
  open it in VS Code at that line; right-click it to reveal it in Finder. Links
  appear only over files that exist.
- **Panes close when their shell does.** `exit` closes the pane, as in iTerm2. A
  non-zero exit leaves it open with its output and a Restart button.
- **607 themes**, from Ghostty's set, with the window chrome derived from each.
- **Phones.** A key bar for what a phone keyboard lacks (Esc, Tab, Ctrl, Alt,
  arrows), no autocorrect in the terminal, and a sidebar that gets out of the way.

---

## Keyboard shortcuts

| | |
|---|---|
| `⌘N` | Open a workspace (pick a project directory) |
| `⌘T` | New tab |
| `⌘D` | Split right |
| `⇧⌘D` | Split down |
| `⌘W` | Close the current pane |
| `⇧⌘[` / `⇧⌘]` | Previous / next tab |
| `⌘B` | Fold / unfold the sidebar |
| `⌘,` | Settings |

### Mouse

| | |
|---|---|
| `⌘`-click a file path | Open it in VS Code (macOS app) |
| Right-click a file path | Reveal in Finder (macOS app) |
| Double-click a workspace / tab title | Rename |
| Right-click a workspace | Menu: Rename / Close |
| Hover a workspace → `×` | Close it and its terminals (asks first) |
| A tab's `×`, or middle-click | Close tab |
| Drag a divider | Resize the split |
| Drag a workspace row / tab | Reorder |

---

## Development

```bash
cd core && cargo run -- --ui ../ui/dist    # daemon, serving the UI from disk
cd ui   && pnpm dev                        # frontend hot reload, proxies to :17788
./mac/scripts/build-app.sh                 # → mac/build/BeeBox.app
./mac/scripts/run.sh                       # build + relaunch "BeeBox Dev": its own bundle id and
                                           # ~/.beebox-dev, so it runs beside an installed BeeBox
./mac/scripts/check.sh                     # full test suite (incl. desktop self-test)
cd android && ./gradlew assembleRelease    # → app/build/outputs/apk/release
```

The desktop shell is a thin Swift `WKWebView` layer built with SwiftPM — no
Xcode needed; `build-app.sh` assembles the `.app` by hand. The Android app is a
Kotlin `WebView` shell.

### Fonts

The terminal draws box-drawing and Powerline glyphs that phones don't ship.
`ui/public/fonts/` holds a subset of MesloLGS NF (9.8 MB TTF → 565 KB WOFF2).
Regenerate:

```bash
cd ui && python3 scripts/build-font.py
```

### Themes

```bash
git clone --depth 1 https://github.com/mbadolato/iTerm2-Color-Schemes.git
cd ui && python3 scripts/build-themes.py ../iTerm2-Color-Schemes/ghostty
```

### Code layout

```
core/src/
  proto.rs          wire protocol, the single source of truth
  session.rs        workspace/tab/split tree
  share.rs          the two authorization predicates
  pty.rs            PTY registry, batching, fan-out
  ring.rs           line-based scrollback ring
  modes.rs          mode sniffing (not a VT parser)
  store.rs          SQLite
  app.rs            state and mutation ops
  http.rs           axum: /ws, /a /w /t /p, /hooks
  agent*.rs         agent hooks and adapters (Claude Code, Codex)
ui/src/
  lib/proto.ts           TS mirror of proto.rs
  lib/conn.ts            WebSocket + MessagePack + reconnect heartbeat
  lib/state.svelte.ts    state + terminal registry
  lib/file-links.ts      file paths in terminal output
  lib/components/        Sidebar, TabBar, SplitTree, Pane, dialogs
mac/
  Sources/BeeBoxShell/   Swift WKWebView shell: the window, the menu, the daemon as a child
  scripts/               build-app.sh, run.sh, check.sh
android/                 Kotlin WebView shell: QR scanning, key bar
```

---

## Roadmap

- [ ] **Claude ↔ Codex handoff**: hand one agent's context to another.
- [ ] **Image upload via `@path`**.
- [ ] **Native Apple app** for iPad and foldables.
- [ ] **Plugins**: a protocol and runtime; the titlebar already reserves a slot.
