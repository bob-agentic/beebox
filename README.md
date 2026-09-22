<div align="center">

# 🐝 BeeBox

**A web terminal built for running many agent CLIs in parallel.**

`v0.1.2`

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
terminal it adds exactly two things: a **status dot per pane**, driven by agent
hooks, and the ability to **share any level of a session over a URL**.

UI prototype: [proto/index.html](proto/index.html)

---

## Two ways to run it

### Desktop app (macOS · Apple Silicon)

```bash
cd ui && pnpm install && pnpm build       # frontend first, produces dist
./mac/scripts/build-app.sh                # one command → mac/build/BeeBox.app
open mac/build/BeeBox.app
```

**Apple Silicon (`arm64`) only** — Intel Macs are not a supported target.

Self-contained — frontend, fonts, and daemon all live inside the app, no
external dependencies. The desktop shell is a thin Swift `WKWebView` layer (no
longer Tauri); the daemon (`beebox-core`) is spawned as a child process, listens
on loopback only, and **quitting the app takes every terminal with it — no
orphaned processes left behind**.

> No Xcode required: `build-app.sh` uses SwiftPM to produce the executable and
> assembles the `.app` by hand.

### Headless daemon (remote access)

```bash
cd core && cargo build --release
./target/release/beebox-core --ui ../ui/dist
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
> links, enforces four scopes (All / Workspace / Tab / Pane), issues pairing
> codes, and lets you see and kick live connections. **How you actually get that
> port in front of another person — a tunnel, frp,
> [Tailscale](https://tailscale.com),
> [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/),
> a reverse proxy, or anything else — BeeBox does not decide for you, and ships
> no tunneling of its own.**
>
> You're an engineer; you know how to expose a local port. Pick whatever suits
> you — **the last mile is yours to complete.** What BeeBox owns is what happens
> once someone can reach the port: how access is scoped, and how a share is
> revoked.

### Flags

| Flag | Default | Meaning |
|---|---|---|
| `--listen` | `0.0.0.0:17788` | Bind address. How it's exposed is up to you; no restrictions imposed. |
| `--home` | `~/.beebox` | State directory (the layout database). |
| `--scrollback` | `200000` | Scrollback lines per pane. |
| `--ui` | (embedded) | Serve the frontend from disk. For development. |
| `--exit-with-parent` | off | Exit when the parent process does (the desktop shell uses this to own the daemon's lifetime). |

### Development

```bash
cd core && cargo run -- --ui ../ui/dist    # backend
cd ui   && pnpm dev                        # frontend hot reload, proxies to :17788
./mac/scripts/build-app.sh                 # one command → mac/build/BeeBox.app
./mac/scripts/check.sh                     # full test suite (incl. desktop self-test)
```

### Fonts

The terminal draws box-drawing and Powerline glyphs that phones don't ship.
`ui/public/fonts/` holds a subset of MesloLGS NF (9.8 MB TTF → 565 KB WOFF2).
Regenerate:

```bash
cd ui && python3 scripts/build-font.py
```

### Themes

**607 of them**, from Ghostty's theme set (i.e. iTerm2-Color-Schemes), with a
full ANSI 16-color palette. Searchable and filterable by light/dark in the
settings panel. Regenerate:

```bash
git clone --depth 1 https://github.com/mbadolato/iTerm2-Color-Schemes.git
cd ui && python3 scripts/build-themes.py ../iTerm2-Color-Schemes/ghostty
```

The window chrome color is derived from each theme's background, so none of the
600 themes ever produces a window that clashes with its terminal.

---

## Keyboard shortcuts

| | |
|---|---|
| `⌘N` | New workspace — **opens a directory picker** (pick a project folder to open). |
| `⌘T` | New tab |
| `⌘D` | Split vertically (left/right) |
| `⇧⌘D` | Split horizontally (top/bottom) |
| `⌘W` | Close the current pane |
| `⇧⌘[` / `⇧⌘]` | Previous / next tab (same as Chrome) |
| `⌘,` | Appearance settings |

### Mouse (matching mux0)

| | |
|---|---|
| Double-click a workspace / tab title | Rename |
| Right-click a workspace | Menu: Rename / Close |
| Hover a workspace → `×` | Quick close (takes all its terminals; asks first) |
| A tab's `×`, or middle-click | Close tab |
| Drag a divider | Resize the split |
| Drag a workspace row / tab | Reorder (persisted) |

> **A workspace is a project directory.** On a fresh install with no workspace,
> BeeBox pops a picker to open one — it never guesses implicitly.

---

## Done

- **A real terminal**: PTY pool, raw byte passthrough, 200k-line scrollback,
  correct CJK/emoji.
- **Matrix structure**: workspace × tab × split tree (iTerm2-style, capped at 2 levels).
- **Layout persistence**: SQLite, restored on restart.
- **Mode sniffing**: alt screen / bracketed paste / cursor-key mode restored on replay.
- **Four-scope sharing**: All / Workspace / Tab / Pane, with a two-tier pairing-code strategy.
- **Connection management**: visible, and kickable (in place of expiry timers).
- **Desktop client**: thin Swift `WKWebView` shell, daemon as a child process,
  frontend embedded, honeycomb `.icns` icon.
- **Background terminal survival**: terminals in tabs/workspaces you switch away
  from are not destroyed; come back and the content is still there.
- **Agent hooks**: six-state status dots for Claude Code / Codex, tool detail,
  completion summaries, auto tab titles, an Agents settings page, and
  auto-resume on restart. OpenCode and bash/fish are explicitly skipped.
- **Workspace interaction**: open = pick a directory, auto-prompt on cold start,
  right-click menu (Rename / Close).

## Roadmap / TODO

### Sharing pipeline
- [ ] **Land grants on real connections**: wire a generated share link through to an actual WebSocket session.
- [ ] **Pairing-code exchange**: the full flow for the two-tier strategy.
- [ ] **Peer reporting**: live sync of the connection list.

### Agent collaboration
- [ ] **Claude ↔ Codex handoff**: hand one agent's context off to another.

### Mobile & native apps
- [ ] **Responsive mobile layout**: phone/tablet.
- [ ] **Image upload via `@path`**.
- [ ] **Native Android app**: targeting foldables (folded/unfolded dual-form adaptation).
- [ ] **Native Apple app**: targeting foldables / iPad multi-form.

### Plugin system
- [ ] **Plugin protocol research**: evaluate compatibility with an existing
      plugin-market protocol (e.g. the Herd/Herdr marketplace), so the ecosystem's
      plugins can be reused rather than all built from scratch. The titlebar already
      reserves a slot for plugins.
- [ ] **Plugin runtime & extension points**: settle the permission model and loading mechanism.

---

## Code layout

```
core/src/
  proto.rs     wire protocol, the single source of truth
  session.rs   workspace/tab/split tree
  share.rs     the two authorization predicates
  pty.rs       PTY registry, batching, fan-out
  ring.rs      line-based scrollback ring
  modes.rs     mode sniffing (not a VT parser)
  store.rs     SQLite
  app.rs       state and mutation ops
  http.rs      axum: /ws, /a /w /t /p, /hooks
ui/src/
  lib/proto.ts           TS mirror of proto.rs
  lib/conn.ts            WebSocket + MessagePack + reconnect heartbeat
  lib/state.svelte.ts    state + terminal registry
  lib/components/        Sidebar, TabBar, SplitTree, Pane, ContextMenu, dialogs
mac/
  Sources/BeeBoxShell/   Swift WKWebView shell: opens the window + runs the daemon as a child
  scripts/build-app.sh   assembles the .app without Xcode
  icons/                 icon source (.icns generated by make-icns.sh)
```
