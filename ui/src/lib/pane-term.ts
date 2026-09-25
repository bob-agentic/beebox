// One pane's terminal, kept by the store for as long as the pane exists.
//
// The Pane component comes and goes more often than its pane does: splitting
// a pane, or closing its split, moves it to another place in the layout, and
// Svelte builds it afresh there. The Terminal cannot go with it — its buffer
// is the history you were reading, and a failed command's output is the one
// thing the server may no longer have a process for. xterm opens a terminal
// once, so a new mount moves the terminal's element into its own host.
//
// What belongs to the terminal lives here; what belongs to a place on screen
// (measuring, observing the host) stays with the component.

import { Terminal, type ITerminalAddon } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import { settings } from './settings.svelte';
import { fileLinkProvider } from './file-links';
import { sentRows } from './sent-messages';

/** The Mac shell's WKWebView, which needs its own renderer and fixes. */
const macShell = '__BEEBOX__' in window;

export interface TermWiring {
  /** What the user typed, as xterm hands it over. */
  data(s: string): void;
  binary(bytes: Uint8Array): void;
  /** The pane's directory now, for resolving relative paths in links. */
  cwd(): string;
  /** Whether Claude Code or Codex has run here, which is where there are
      sent messages to step between. */
  agent(): boolean;
}

export class PaneTerm {
  readonly term: Terminal;
  readonly fit = new FitAddon();
  /** The mounted component's re-fit. A no-op while between mounts. */
  report: (force?: boolean) => void = () => {};
  /** The host the terminal is in now, to tell a stale unmount from ours. */
  private host: HTMLElement | null = null;
  private serializer: { serialize(): string } | null = null;
  private refreshFrame = 0;
  /** A renderer only while on screen; hidden panes fall back to xterm's DOM
      renderer, which costs next to nothing when not drawn. In a browser the
      limit is GL contexts — eight or so on Android, the oldest dropped past
      that — so with every tab mounted the pane you were reading lost its
      WebGL. In the Mac shell it is memory: each Canvas2D pane holds four
      full-size canvases at 2x, and ten mounted panes came to ~700 MB. */
  private renderer: ITerminalAddon | null = null;
  private makeRenderer: (() => ITerminalAddon) | null = null;
  private wantRenderer = false;
  private stopWatching: () => void;
  /** The sent message the last ⌘↑/⌘↓ landed on, and where that left the
      viewport. Scrolling anywhere else starts the next step afresh. */
  private stepped: { row: number; viewport: number } | null = null;

  constructor(wiring: TermWiring) {
    const cfg = settings.current;
    const term = new Terminal({
      fontFamily: cfg.fontFamily,
      fontSize: cfg.fontSize,
      lineHeight: cfg.lineHeight,
      cursorBlink: cfg.cursorBlink,
      allowProposedApi: true,
      // How much history this browser holds ready to scroll through. Costs
      // about 2 MB per ten thousand lines, per pane, and every pane pays it
      // whether or not its tab is in front — which is why it is a setting
      // rather than the ring's full 200k. The daemon keeps everything either
      // way; this is the client's share.
      scrollback: cfg.scrollback,
      theme: settings.xterm,
    });
    this.term = term;
    term.loadAddon(this.fit);

    // Lets the end-to-end tests read what the terminal is actually showing.
    // The renderers draw to a canvas, so there is nothing in the DOM to assert on.
    void import('@xterm/addon-serialize').then(({ SerializeAddon }) => {
      const ser = new SerializeAddon();
      term.loadAddon(ser);
      this.serializer = ser;
    });
    term.loadAddon(new Unicode11Addon());
    // Correct widths for CJK and emoji — verified on a real device.
    term.unicode.activeVersion = '11';

    // A replay marks the width each stretch of history was written at
    // (`width_marker` in core/src/pty.rs). Drawn at any other, the redraws in
    // it erase the wrong rows and leave old frames behind. The handler runs in
    // stream order, and the last marker is the PTY's width now.
    term.parser.registerOscHandler(7788, (cols) => {
      if (+cols > 0 && +cols !== term.cols) term.resize(+cols, term.rows);
      return true;
    });

    term.onWriteParsed(() => this.refresh());

    // ⌘-click a file path to open it in VS Code. Only in the Mac shell: it
    // is the one client on the machine whose disk the paths are on, and it
    // checks each one — a link appears only over a file that exists.
    if (macShell) {
      term.registerLinkProvider(fileLinkProvider(term, (window as any).__BEEBOX__, wiring.cwd));
    }

    // WebKit currently accepts the WebGL context but composites it as a blank
    // layer. Browsers keep the accelerated renderer; the native shell uses
    // xterm's Canvas2D addon, which WebKit snapshots and composites reliably.
    void (macShell
      ? import('@xterm/addon-canvas').then(({ CanvasAddon }) => () => new CanvasAddon())
      : import('@xterm/addon-webgl').then(({ WebglAddon }) => () => {
          const addon = new WebglAddon();
          addon.onContextLoss(() => {
            addon.dispose();
            if (this.renderer === addon) this.renderer = null;
          });
          return addon;
        })
    ).then((make) => {
      this.makeRenderer = make;
      this.show(this.wantRenderer);
    });

    // `onData` is the user's own input — what the process prints never reaches
    // it — so it is the right place to say "this is the tab I am working in".
    term.onData((s) => wiring.data(s));
    term.onBinary((s) => {
      const bytes = new Uint8Array(s.length);
      for (let i = 0; i < s.length; i++) bytes[i] = s.charCodeAt(i) & 0xff;
      wiring.binary(bytes);
    });

    // ⌘↑/⌘↓ step between the messages you sent. Taken before xterm sees the
    // key, which would otherwise type it into the agent as an escape sequence.
    term.attachCustomKeyEventHandler((e) => {
      const dir = e.key === 'ArrowUp' ? -1 : e.key === 'ArrowDown' ? 1 : 0;
      if (!dir || !e.metaKey || e.shiftKey || e.altKey || e.ctrlKey) return true;
      if (!wiring.agent()) return true;
      if (e.type === 'keydown') this.step(dir);
      e.preventDefault();
      return false;
    });

    // Appearance changes apply to terminals that already exist, so you can see
    // a font or theme land without restarting anything.
    this.stopWatching = settings.onChange(() => {
      const c = settings.current;
      term.options.fontFamily = c.fontFamily;
      term.options.fontSize = c.fontSize;
      term.options.lineHeight = c.lineHeight;
      term.options.cursorBlink = c.cursorBlink;
      // Applies immediately, both ways: raising it lets the buffer grow from
      // here on, lowering it trims what is already there.
      term.options.scrollback = c.scrollback;
      term.options.theme = settings.xterm;
      // Glyph size changed, so the column count did too.
      this.report();
    });
  }

  /** Puts the terminal in `host`: opened there the first time, moved there
      after. `report` is that mount's re-fit. */
  attach(host: HTMLElement, report: (force?: boolean) => void) {
    this.host = host;
    this.report = report;
    const el = this.term.element;
    if (!el) {
      this.term.open(host);
      this.patchIme(this.term.element!);
      return;
    }
    // Moving an element loses its scroll offset, and xterm only writes the
    // offset back when it thinks it changed — so the viewport would sit at the
    // top of the history while the buffer still says the bottom, and the next
    // scroll would jump there. Stepping one line and back makes xterm sync it.
    const buf = this.term.buffer.active;
    const y = buf.viewportY;
    host.appendChild(el);
    if (buf.baseY > 0) {
      this.term.scrollToLine(y > 0 ? y - 1 : y + 1);
      this.term.scrollToLine(y);
    }
    if (this.term.rows > 0) this.term.refresh(0, this.term.rows - 1);
  }

  /** Called as a mount goes. A later mount may already have taken over. */
  detach(host: HTMLElement) {
    if (this.host !== host) return;
    this.host = null;
    this.report = () => {};
  }

  /** Scrolls to the sent message before (-1) or after (1) the one last
      stepped to, or, starting fresh, the last one above the bottom of the
      screen / the first one below its top. Past the newest is the bottom. */
  step(dir: -1 | 1) {
    const buf = this.term.buffer.active;
    const rows = sentRows(buf);
    const s = this.stepped;
    const from =
      s && s.viewport === buf.viewportY
        ? s.row
        : dir < 0
          ? buf.viewportY + this.term.rows
          : buf.viewportY;
    let row: number | undefined;
    if (dir < 0) {
      for (const r of rows) if (r < from) row = r;
    } else {
      row = rows.find((r) => r > from);
    }
    if (row === undefined) {
      if (dir > 0) {
        this.scrollLines(buf.baseY - buf.viewportY);
        this.stepped = null;
      }
      return;
    }
    // Near the bottom the viewport cannot put the row at its top.
    const viewport = Math.min(row, buf.baseY);
    this.scrollLines(viewport - buf.viewportY);
    this.stepped = { row, viewport };
    this.flash(row);
  }

  /** By wheel, not scrollToLine: on a phone the viewport does not follow
      scrollToLine at all (see `jump` in state.svelte.ts). */
  private scrollLines(n: number) {
    if (!n) return;
    this.term.element?.querySelector('.xterm-viewport')?.dispatchEvent(
      new WheelEvent('wheel', {
        deltaY: n,
        deltaMode: WheelEvent.DOM_DELTA_LINE,
        bubbles: true,
        cancelable: true,
      }),
    );
  }

  /** Marks the line stepped to for a moment, so the eye finds it. */
  private flash(row: number) {
    const buf = this.term.buffer.active;
    const marker = this.term.registerMarker(row - (buf.baseY + buf.cursorY));
    if (!marker) return;
    const deco = this.term.registerDecoration({ marker, width: this.term.cols });
    deco?.onRender((el) => el.classList.add('sent-flash'));
    setTimeout(() => marker.dispose(), 1200);
  }

  serialize(): string {
    return this.serializer?.serialize() ?? '';
  }

  /** Loads or drops the renderer as the pane comes on or off screen. */
  show(on: boolean) {
    this.wantRenderer = on;
    if (!this.makeRenderer) return; // still loading; applied when it lands
    if (!on) {
      this.renderer?.dispose();
      this.renderer = null;
      return;
    }
    if (this.renderer || !this.term.element) return;
    try {
      const addon = this.makeRenderer();
      this.term.loadAddon(addon);
      this.renderer = addon;
      this.refresh();
    } catch {
      // The built-in DOM renderer remains active.
    }
  }

  /** WKWebView can parse output into the buffer without invalidating xterm's
      compositing layer. Coalesce an explicit refresh to the next frame so a
      busy agent still causes at most one extra paint per display frame.
      Only there: elsewhere a full repaint per frame of output is pure cost,
      and on a phone it fights scrolling for the same frames. */
  refresh() {
    if (!macShell || this.refreshFrame) return;
    this.refreshFrame = requestAnimationFrame(() => {
      this.refreshFrame = 0;
      if (this.term.rows > 0) this.term.refresh(0, this.term.rows - 1);
    });
  }

  dispose() {
    this.stopWatching();
    if (this.refreshFrame) cancelAnimationFrame(this.refreshFrame);
    this.renderer?.dispose();
    this.renderer = null;
    this.report = () => {};
    this.term.dispose();
  }

  // WebKit drops the first Chinese full-width punctuation mark: `？` needs
  // two presses, while Han characters are fine.
  //
  // xterm only accepts an `insertText` when `_keyDownSeen` is false, assuming
  // the `input` for a keystroke arrives after its `keydown`. WebKit reverses
  // that pair for IME direct-commit, and these marks need Shift — whose own
  // keydown set the flag and whose keyup has not run yet — so the character
  // is discarded. Han characters go through compositionend instead, which
  // never consults the flag. Upstream: xtermjs/xterm.js#6144, still open.
  //
  // Clearing the flag in `beforeinput`, which always precedes the `input`
  // xterm listens for, lets the character through its normal path. Nothing
  // extra is sent, and the duplicate-suppression xterm already does is
  // untouched — so this cannot double up. Composition is left strictly alone.
  //
  // On the terminal's own element rather than the host, so it moves with it.
  private patchIme(el: HTMLElement) {
    if (!macShell) return;
    el.addEventListener(
      'beforeinput',
      (ev: Event) => {
        const e = ev as InputEvent;
        if (e.inputType !== 'insertText' || !e.data) return;
        const core = (this.term as any)._core;
        if (core && !core._compositionHelper?.isComposing) {
          core._keyDownSeen = false;
        }
      },
      true,
    );
  }
}
