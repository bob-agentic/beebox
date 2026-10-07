// Application state. Mirrors the server's TreeView and routes output to the
// terminals, which it keeps per pane for as long as the pane exists — a Pane
// component is rebuilt whenever the layout around it changes, and its history
// must outlive that (see pane-term.ts).

import { SvelteSet } from 'svelte/reactivity';
import { markRead, pruneRead } from './agent-status';
import { Conn } from './conn';
import { countdown, type Timer } from './countdown';
import { settings } from './settings.svelte';
import { fixPartialLineMarkers } from './partial-line';
import { PaneTerm } from './pane-term';
import type {
  AgentStatusView,
  Caps,
  Dir,
  In,
  Out,
  PaneId,
  PaneView,
  Peer,
  Shelf,
  TabId,
  TreeView,
  WsId,
} from './proto';

/** How long the status bar says who just joined. */
export const PAIRED_MS = 5000;

const EMPTY_TREE: TreeView = { workspaces: [], active_ws: null, active_tab: null };

/** This browser's lasting identity for share links, made on first use. Lose
    it (cleared site data) and the links it held have to be shared again. */
function deviceSecret(): string {
  const KEY = 'beebox.device';
  let secret = localStorage.getItem(KEY);
  if (!secret) {
    const bytes = crypto.getRandomValues(new Uint8Array(16));
    secret = Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
    localStorage.setItem(KEY, secret);
  }
  return secret;
}

class Store {
  tree = $state<TreeView>(EMPTY_TREE);
  caps = $state<Caps>({
    writable: false,
    host: false,
    may_open_tab: false,
    show_sidebar: false,
    show_tabs: false,
  });
  connected = $state(false);
  /** Why the server closed us, if it did. */
  closedReason = $state<string | null>(null);
  peers = $state<Peer[]>([]);
  /** Focused pane. Local to this browser: the server has no "current pane". */
  focused = $state<PaneId | null>(null);

  /** True in the phone app, the one client that drives the terminal's size
   *  (`is_phone_app` in core/src/http.rs). Only it has any use for a re-fit
   *  control: everyone else's size is the owner's, and asking for it again
   *  would change nothing. */
  sizing = $state(false);
  share = $state<{ url: string; hosts: string[] } | null>(null);
  /** Whether the daemon serves non-loopback clients. Owner-only knob. */
  webExposed = $state(false);
  private wsBase = '';
  /** Bumped whenever this browser marks a completion read. localStorage is
      not reactive, so aggregate dots (tab, workspace) depend on this to
      re-derive their unread flag. */
  readRev = $state(0);
  /** Whether this window has the user's attention. The focused pane is only
      the one last clicked; with the window behind another app, a run that
      finishes there has not been seen, and must stay unread until you come
      back. */
  windowFocused = $state(document.hasFocus());

  /** The one write path for read-state, so every dot re-derives together.
      Only bumps when something changed — an unconditional bump inside a
      $effect that also reads readRev would loop forever. */
  markStatusRead(pane: PaneId, view: AgentStatusView) {
    if (markRead(pane, view)) this.readRev++;
  }

  /** Terminals by pane id: made when a pane is first shown, disposed when it
      leaves the tree — or when it has been out of sight a while (`release`).
      The daemon sends output only for these; the rest it keeps, and replays
      when a terminal is made again. */
  private terms = new Map<PaneId, PaneTerm>();
  /** Panes whose terminal is still waiting for its history. */
  readonly loading = new SvelteSet<PaneId>();
  /** pty -> pane, so output frames can be routed without a tree lookup. */
  private ptyToPane = new Map<number, PaneId>();
  private conn: Conn | null = null;

  constructor() {
    // Asked afresh on each event rather than taken from its name: WKWebView
    // sends `focus` when the web view takes first responder, even in a window
    // that is not in front.
    const syncFocus = () => (this.windowFocused = document.hasFocus());
    addEventListener('focus', syncFocus);
    addEventListener('blur', syncFocus);
    // In the desktop shell the daemon's address comes from the injected port
    // rather than `location`. It must be spelled `localhost`, not 127.0.0.1:
    // the shell's ATS exception is NSAllowsLocalNetworking, which covers the
    // hostname but not the literal address — and the window loads
    // http://localhost:<port>/, so anything else would also be cross-origin.
    const injectedPort = (window as any).__BEEBOX_PORT__ as number | undefined;
    const host = injectedPort ? `localhost:${injectedPort}` : location.host;
    const proto = injectedPort
      ? 'ws'
      : location.protocol === 'https:'
        ? 'wss'
        : 'ws';
    // A share link carries its token in the path. Otherwise full access needs
    // the owner key, which the server injected into this page only if the URL
    // already proved it — "no token" must never mean "all access", because the
    // daemon listens on every interface by default.
    const m = location.pathname.match(/^\/[awtp]\/(.+)$/);
    const key = (window as any).__BEEBOX_KEY__ as string | undefined;
    // A share link belongs to the first device that opens it; this browser
    // proves it is that device with a secret it keeps for good.
    const q = m
      ? `?token=${encodeURIComponent(m[1])}&device=${deviceSecret()}`
      : key
        ? `?key=${encodeURIComponent(key)}`
        : '';
    // Sizing, for a screen the terminal was not laid out for. A phone shown a
    // 175-column terminal on a 44-column screen gets text overlapping itself —
    // it has to be able to resize the terminal to be readable at all. The cost
    // is that the owner's window resizes with it, so only the phone app does:
    // it says so in its user agent, which the daemon reads too, so a browser
    // cannot opt in by editing a link.
    this.sizing = navigator.userAgent.includes('BeeBoxApp/android');
    const sizing = this.sizing;
    // A resizing client says its size up front, so the replay arrives laid
    // out for the width it will actually use. Saying it only after mounting
    // meant history wrapped for the owner's terminal and re-wrapped here —
    // which is where zsh's reverse-video `%` came from on a phone: the marker
    // is erased by padding to an exact column count, and that count was the
    // other terminal's. An estimate from the window is enough; the precise
    // figure follows from the first fit.
    const guess = sizing ? this.guessSize() : null;
    this.wsBase =
      `${proto}://${host}/ws${q}` +
      (guess ? `${q ? '&' : '?'}cols=${guess.cols}&rows=${guess.rows}` : '');

    this.connect();
  }

  /** Roughly how large a terminal fills this window, before one exists to
   *  measure. Deliberately approximate: it only has to be closer than the
   *  owner's width, and the real size arrives moments later from the fit. */
  private guessSize(): { cols: number; rows: number } | null {
    const cell = { w: 8.4, h: 17 };
    // The chrome around a pane: title bar, tab strip, pane head and foot.
    const chrome = { w: 24, h: 150 };
    const cols = Math.floor((window.innerWidth - chrome.w) / cell.w);
    const rows = Math.floor((window.innerHeight - chrome.h) / cell.h);
    if (cols < 20 || rows < 5) return null;
    return { cols: Math.min(cols, 400), rows: Math.min(rows, 200) };
  }

  private connect() {
    this.conn?.dispose();
    this.conn = new Conn(
      this.wsBase,
      (msg) => this.handle(msg),
      (up) => {
        this.connected = up;
        // A new connection sends nothing until asked. What is on screen stays
        // there until the replay replaces it.
        if (up) for (const pane of this.terms.keys()) this.send({ t: 'replay', pane });
        // A request the old socket took is never answered.
        else for (const [req, done] of this.imageReqs) {
          this.imageReqs.delete(req);
          done(null);
        }
      },
    );
  }

  send(msg: In) {
    this.conn?.send(msg);
  }

  private handle(msg: Out) {
    switch (msg.t) {
      case 'tree': {
        const shown = { ws: this.activeWs?.id ?? null, tab: this.activeTab?.id ?? null };
        this.tree = msg.tree;
        // Anything clicked before the first frame arrived was handled as a
        // viewer would handle it, because `host` starts false — and the local
        // override that leaves behind outranks the server's own active ids
        // forever after, so a new workspace would open behind the old one.
        //
        // A follower keeps its own place until the owner moves: a changed
        // active id is the owner going somewhere, and the follower goes too.
        // Null means the owner is outside this share, so it stays put.
        const owner = `${msg.tree.active_ws}:${msg.tree.active_tab}`;
        const moved = msg.tree.active_tab !== null && owner !== this.ownerAt;
        this.ownerAt = owner;
        if (msg.caps.may_open_tab || moved) {
          this.localWs = null;
          this.localTab = null;
        } else if (msg.tree.active_tab === null && this.localTab === null) {
          // Following an owner who just left the share: pin what is on screen,
          // or the fallback would drop the follower onto the first tab.
          this.localWs = shown.ws;
          this.localTab = shown.tab;
        }
        this.caps = msg.caps;
        this.reconcile();
        break;
      }
      case 'output': {
        // A pty the tree does not name yet is replayed once it does.
        const pane = this.ptyToPane.get(msg.pty);
        const t = pane === undefined ? undefined : this.terms.get(pane);
        t?.term.write(fixPartialLineMarkers(msg.data, t.term.cols, t.line));
        break;
      }
      case 'resync': {
        // Released while the answer was on its way: nothing to put it in.
        const t = this.terms.get(msg.pane);
        if (!t) break;
        // Reset, re-establish terminal modes, then replay. Without the mode
        // prefix the client would send the wrong bytes for arrow keys and
        // pastes — see ARCHITECTURE.md §5a. Shown once written, not before:
        // a long history parses in a few frames, and they should not show.
        // Loading again on a reconnect too, not only the first time: that is
        // also what keeps xterm's answers to the history's queries from being
        // sent as input (see `terminal`).
        this.loading.add(msg.pane);
        t.term.reset();
        t.term.write(msg.modes);
        // A reset terminal starts on an empty line.
        t.line.text = false;
        t.term.write(fixPartialLineMarkers(msg.data, t.term.cols, t.line), () => this.loading.delete(msg.pane));
        break;
      }
      case 'size': {
        const pane = this.pane(msg.pane);
        if (pane) {
          pane.cols = msg.cols;
          pane.rows = msg.rows;
        }
        break;
      }
      case 'status': {
        const pane = this.pane(msg.pane);
        if (pane) pane.status = msg.status;
        break;
      }
      case 'session_title': {
        const pane = this.pane(msg.pane);
        if (pane) pane.session_title = msg.text;
        break;
      }
      case 'web_server': {
        this.webExposed = msg.exposed;
        break;
      }
      case 'title': {
        const pane = this.pane(msg.pane);
        if (pane) pane.title = msg.text;
        break;
      }
      case 'cwd': {
        const pane = this.pane(msg.pane);
        if (pane) {
          pane.cwd = msg.path;
          pane.git = msg.git;
        }
        break;
      }
      case 'exited': {
        const pane = this.pane(msg.pane);
        if (pane) pane.pty = null;
        break;
      }
      case 'peers': {
        // A link someone has just opened, for the first time — not the
        // list this connection starts with, and not a device coming back.
        if (this.peersKnown) {
          for (const p of msg.peers) {
            if (this.peers.some((q) => q.token === p.token) || this.announced.has(p.token)) continue;
            if (p.token === this.awaitingPair) continue;
            this.announced.add(p.token);
            const detail = this.linkLabels.get(p.token) ?? `${p.scope} · ${p.writable ? 'can type' : 'read-only'}`;
            this.announcePairing(`${p.device} connected`, p.addr, detail);
          }
        }
        this.peersKnown = true;
        this.peers = msg.peers;
        break;
      }
      case 'grant':
        this.share = { url: msg.url, hosts: msg.hosts };
        break;
      case 'closed':
        this.connected = false;
        this.closedReason = msg.reason;
        document.title = '404 Not Found';
        // In the app, back to its connect screen instead of a dead page.
        (window as any).__beeboxShell?.linkGone?.();
        break;
      case 'pong':
        break;
      case 'image': {
        const done = this.imageReqs.get(msg.req);
        this.imageReqs.delete(msg.req);
        done?.(msg.data.length ? URL.createObjectURL(new Blob([msg.data as BlobPart], { type: msg.mime })) : null);
        break;
      }
    }
  }

  /** Refreshes pty routing and keeps the focus on a pane that still exists. */
  private reconcile() {
    const live = new Set<PaneId>();
    const was = this.ptyToPane;
    this.ptyToPane = new Map();

    for (const ws of this.tree.workspaces) {
      for (const tab of ws.tabs) {
        for (const p of tab.panes) {
          live.add(p.id);
          if (p.pty === null) continue;
          this.ptyToPane.set(p.pty, p.id);
          // Re-run: the daemon holds back a new process's output until the
          // terminal asks for it.
          if (was.get(p.pty) !== p.id && this.terms.has(p.id)) {
            this.send({ t: 'replay', pane: p.id });
          }
        }
      }
    }
    // The pane is gone, not just moved: its component will not be back.
    for (const [id, t] of this.terms) {
      if (!live.has(id)) this.release(id);
    }
    // Read-state entries for deleted panes go with them.
    pruneRead(live);

    // A split asked for here arrives as a new pane in the split one's tab, and
    // the keyboard goes to it, as in iTerm2: you split to work in the new one.
    const split = this.splitFrom;
    if (split) {
      const tab = this.tree.workspaces
        .flatMap((w) => w.tabs)
        .find((t) => t.panes.some((p) => p.id === split.pane));
      const fresh = tab?.panes.find((p) => !split.had.has(p.id));
      if (fresh) this.focused = fresh.id;
      if (fresh || !tab || performance.now() > split.until) this.splitFrom = null;
    }

    // Focus follows the visible tab. Without this, opening a tab leaves focus
    // on the previous one, and ⌘D then splits a pane you cannot see.
    const visible = new Set(this.visiblePanes().map((p) => p.id));
    const before = this.focused;
    if (this.focused === null || !visible.has(this.focused)) {
      this.focused = this.visiblePanes()[0]?.id ?? null;
    }
    // Only when it actually moved. Every tree frame passes through here —
    // a title change, a cwd poll — and re-asserting the keyboard on each one
    // would pull the user out of whatever they were typing in.
    if (this.focused !== before) this.applyFocus();
  }

  /** The pane's terminal, made on first use. */
  terminal(pane: PaneId): PaneTerm {
    let t = this.terms.get(pane);
    if (t) return t;
    // Nothing is typed into a terminal still hidden for its replay: what it
    // sends then is xterm answering queries in the history (`ESC[c` from a
    // program long gone), which would land in whatever runs now as `1;2c`.
    t = new PaneTerm({
      data: (s) => {
        if (this.loading.has(pane)) return;
        this.noteTyping();
        this.send({ t: 'input', pane, data: new TextEncoder().encode(this.withMods(s)) });
      },
      binary: (data) => {
        if (!this.loading.has(pane)) this.send({ t: 'input', pane, data });
      },
      cwd: () => this.pane(pane)?.cwd ?? '',
      agent: () => this.isAgent(pane),
      image: (n, row) => this.openImage(pane, n, row),
    });
    this.terms.set(pane, t);
    this.loading.add(pane);
    return t;
  }

  /** Lets go of a hidden pane's terminal — a terminal's memory is its
      scrollback, some 24 MB per ten thousand lines at 175 columns, and most
      panes are in tabs nobody is looking at. The daemon has all of it, even
      for a process that has ended, and replays it when the pane is shown. */
  release(pane: PaneId) {
    const t = this.terms.get(pane);
    if (!t) return;
    t.dispose();
    this.terms.delete(pane);
    this.loading.delete(pane);
    this.send({ t: 'release', pane });
  }

  /** Called by a Pane as it mounts, to put the pane's terminal on screen. */
  attach(pane: PaneId, host: HTMLElement, report: (force?: boolean) => void) {
    this.terminal(pane).attach(host, report);
    // The tree frame that chose this pane may have arrived before the element
    // did, in which case applyFocus found nothing to focus. This is the other
    // half of that race — without it the first pane after a cold start still
    // needs a click.
    if (this.focused === pane) this.applyFocus();
  }

  /** Bumped on every keystroke sent to a terminal.
   *
   *  Typing is the clearest statement that this tab is the one you are working
   *  in — so the tab strip watches this to scroll back to it. Having scrolled
   *  away to look at another tab and then started typing, the tab you are
   *  actually in is the one that should be on screen. A counter rather than a
   *  flag: the point is that it *changed*, not what it holds. */
  typedRev = $state(0);

  /** CTRL and ALT from the phone's key bar. One-shot, as in Termux: they
   *  apply to the next key typed, from the bar or the soft keyboard, and
   *  then let go. */
  mods = $state({ ctrl: false, alt: false });

  /** Applies and releases the key bar's modifiers. Every keystroke passes
   *  through here on its way to the pty. */
  withMods(s: string): string {
    const { ctrl, alt } = this.mods;
    if (!ctrl && !alt) return s;
    this.mods = { ctrl: false, alt: false };
    if (ctrl && s.length === 1) {
      const c = s.toUpperCase().charCodeAt(0);
      if (c >= 64 && c <= 95) s = String.fromCharCode(c - 64);
      else if (s === ' ') s = '\0';
      else if (s === '?') s = '\x7f';
    }
    return alt ? '\x1b' + s : s;
  }

  /** Types a key-bar key into the pane on screen. `app` is its spelling in
   *  application cursor mode, which vim and less switch on. */
  typeKey(seq: string, app?: string) {
    const pane = this.targetPane();
    const t = pane === null ? undefined : this.terms.get(pane);
    if (!t) return;
    t.term.input(app && t.term.modes.applicationCursorKeysMode ? app : seq);
  }

  /** Called by a pane when the user types into it. */
  noteTyping() {
    this.typedRev++;
  }

  focus(pane: PaneId) {
    this.focused = pane;
    this.applyFocus();
  }

  /** A split this window asked for: the pane split, the panes its tab had
      then, and until when to wait for the new one — a refused split (the
      depth limit) never brings one, and a pane added later by someone else
      must not take the keyboard. */
  private splitFrom: { pane: PaneId; had: Set<PaneId>; until: number } | null = null;

  /** Splits a pane, and moves the keyboard into the new one when it comes. */
  split(pane: PaneId, dir: Dir) {
    const tab = this.tree.workspaces.flatMap((w) => w.tabs).find((t) => t.panes.some((p) => p.id === pane));
    this.splitFrom = { pane, had: new Set(tab?.panes.map((p) => p.id)), until: performance.now() + 3000 };
    this.send({ t: 'split', pane, dir });
  }

  /** The pane a split/close should act on: the focused one, but only if it is
   *  actually on screen.
   *
   *  `focused` is a bare id with no workspace attached, and `reconcile` only
   *  re-checks it when a tree frame arrives. Any path that changes what is
   *  visible without producing one leaves it pointing into the workspace you
   *  just left — and ⌘D then splits a pane there, which is how a fin-baker-svc
   *  terminal appeared inside bee-box. Resolving it against the visible set
   *  makes that unrepresentable rather than merely unlikely. */
  targetPane(): PaneId | null {
    const visible = this.visiblePanes();
    if (this.focused !== null && visible.some((p) => p.id === this.focused)) {
      return this.focused;
    }
    const fallback = visible[0]?.id ?? null;
    if (fallback !== this.focused) {
      this.focused = fallback;
      this.applyFocus();
    }
    return fallback;
  }

  /** Puts the keyboard where `focused` already points.
   *
   *  Separate from `focus()` so a tree update can re-assert the keyboard
   *  without re-deciding whose turn it is. Callers are responsible for only
   *  invoking it when the focused pane actually changed; this method guards
   *  against stealing focus, not against being called too often.
   *
   *  Panes are never unmounted — switching tabs only toggles a class, so the
   *  scrollback survives — which is why this cannot live in an onMount. */
  private applyFocus() {
    if (this.focused === null) return;
    const t = this.terms.get(this.focused);
    // Not mounted yet; register() finishes the job.
    if (!t) return;
    const active = document.activeElement;
    // Already here. Re-focusing would be a no-op in the DOM but still churns
    // xterm's focus bookkeeping.
    if (active === t.term.textarea) return;
    // Someone is typing somewhere that is not a terminal — a rename box, a
    // settings field. A tab switch behind a dialog must not yank the caret
    // out of it.
    if (
      active instanceof HTMLElement &&
      (active.tagName === 'INPUT' || active.tagName === 'TEXTAREA') &&
      !active.classList.contains('xterm-helper-textarea')
    ) {
      return;
    }
    t.term.focus();
  }

  /** Jumps one pane's viewport to the oldest or newest line it holds.
   *
   *  Scrollback runs to tens of thousands of lines, which on a phone is more
   *  flicks than anyone will make. The terminal already knows how to get
   *  there in one step. */
  jump(pane: PaneId, where: 'top' | 'bottom') {
    const t = this.terms.get(pane);
    if (!t) return;
    // Not scrollToTop/scrollToBottom: measured on a device, those move the
    // viewport not at all while a wheel event moves it all the way. xterm
    // honours the wheel and nothing else — the same finding that made touch
    // scrolling work. A delta past any possible height lands on the end.
    const el = (t.term as unknown as { element?: HTMLElement }).element;
    const vp = el?.querySelector('.xterm-viewport');
    vp?.dispatchEvent(
      new WheelEvent('wheel', {
        deltaY: where === 'top' ? -1e7 : 1e7,
        deltaMode: 0,
        bubbles: true,
        cancelable: true,
      }),
    );
  }

  /** The same jump, aimed at whichever pane is on screen. The title bar has
   *  no pane of its own, and a phone shows one at a time. */
  jumpVisible(where: 'top' | 'bottom') {
    const pane = this.targetPane();
    if (pane !== null) this.jump(pane, where);
  }

  /** Claude Code or Codex has run in this pane — what its hooks last said. */
  isAgent(pane: PaneId | null): boolean {
    const a = pane === null ? null : this.pane(pane)?.agent;
    return a === 'claude' || a === 'codex';
  }

  /** A device that has just opened a share link, said over the status bar's
      share count — where it can be managed from — for a few seconds. `from`
      is the address it came from. */
  paired = $state<{ id: number; title: string; from: string | null; detail: string } | null>(null);
  /** The pointer is on the note: it stays until it leaves. */
  pairedHeld = $state(false);
  private pairedTimer: Timer | null = null;
  /** Whether the first peer list has come: it is what already was. */
  private peersKnown = false;
  /** Links whose pairing has been told already. */
  readonly announced = new Set<string>();
  /** What each link made here was for, in the share dialog's words —
      "This tab · bbimg · read-only" — for when it is opened later. The
      daemon only knows the kind of scope, not its name. */
  readonly linkLabels = new Map<string, string>();
  /** The link the share dialog is showing, which it announces itself. */
  awaitingPair: string | null = null;

  announcePairing(title: string, from: string | null, detail: string) {
    this.pairedTimer?.cancel();
    this.paired = { id: (this.paired?.id ?? 0) + 1, title, from, detail };
    this.pairedHeld = false;
    this.pairedTimer = countdown(PAIRED_MS, () => {
      this.paired = null;
      this.pairedTimer = null;
    });
  }

  holdPaired(on: boolean) {
    this.pairedHeld = on;
    this.pairedTimer?.hold(on);
  }

  /** The image open in the viewer. `url` is null while it loads, and stays
      null with `missing` when the transcript has no such image. */
  image = $state<{
    pane: PaneId;
    n: number;
    row: string;
    url: string | null;
    missing: boolean;
    /** What was asked with it, from the terminal; null once that has
        scrolled out of the terminal's history. */
    asked: string | null;
  } | null>(null);
  private imageReqs = new Map<number, (url: string | null) => void>();
  private nextImageReq = 1;
  /** Images already fetched, as blob URLs, oldest first. */
  private imageUrls = new Map<string, string>();

  private fetchImage(pane: PaneId, n: number, row: string): Promise<string | null> {
    const key = `${pane}:${n}:${row}`;
    const have = this.imageUrls.get(key);
    if (have) return Promise.resolve(have);
    const req = this.nextImageReq++;
    this.send({ t: 'image', pane, n, row, req });
    return new Promise((resolve) => {
      this.imageReqs.set(req, (url) => {
        if (url) {
          this.imageUrls.set(key, url);
          // A screenshot is some hundreds of KB; a few dozen is plenty.
          for (const [k, old] of this.imageUrls) {
            if (this.imageUrls.size <= 40) break;
            if (old === this.image?.url) continue;
            URL.revokeObjectURL(old);
            this.imageUrls.delete(k);
          }
        }
        resolve(url);
      });
    });
  }

  private asked(pane: PaneId, n: number, row: string): string | null {
    return this.terms.get(pane)?.imageMessage(n, row)?.text || null;
  }

  openImage(pane: PaneId, n: number, row: string) {
    const view = { pane, n, row, url: null, missing: false, asked: this.asked(pane, n, row) };
    this.image = view;
    void this.fetchImage(pane, n, row).then((url) => {
      if (this.image?.pane !== pane || this.image.n !== n) return;
      this.image = { ...view, url, missing: !url };
    });
  }

  /** The image before or after the open one: the session's next number for
      Claude, the message's for Codex — the same row picks the same message. */
  async stepImage(dir: -1 | 1): Promise<boolean> {
    const cur = this.image;
    if (!cur || cur.n + dir < 1) return false;
    const n = cur.n + dir;
    const url = await this.fetchImage(cur.pane, n, cur.row);
    if (!url || this.image !== cur) return false;
    this.image = { ...cur, n, url, missing: false, asked: this.asked(cur.pane, n, cur.row) };
    return true;
  }

  /** Closes the viewer on the message the open image was sent in. Looked
      up afresh: output may have moved it since the viewer opened. */
  locateImage() {
    const cur = this.image;
    if (!cur) return;
    const t = this.terms.get(cur.pane);
    const found = t?.imageMessage(cur.n, cur.row);
    this.image = null;
    if (t && found) t.reveal(found.row);
  }

  closeImage() {
    this.image = null;
  }

  /** ⌘↑/⌘↓ for the pane on screen, from the phone's key bar. */
  stepSent(dir: -1 | 1) {
    const pane = this.targetPane();
    if (pane !== null) this.terms.get(pane)?.step(dir);
  }

  /** Asks every mounted pane to re-measure and say so, even if the numbers
   *  come out the same, then rebuilds it: replayed, and redrawn by its
   *  program, which is what cleans up a screen drawn wrong. Bound to the
   *  re-fit button, which only a client that drives its own size gets to see. */
  refit() {
    for (const [pane, t] of this.terms) {
      t.report(true);
      this.loading.add(pane);
      this.send({ t: 'replay', pane });
    }
  }

  /** True while the layout is animating, so terminals keep their size until
   *  it settles instead of re-fitting on every frame. */
  holdFit = false;
  private holdTimer = 0;

  /** Holds every terminal's size for `ms`, then fits each once. */
  holdFitFor(ms: number) {
    this.holdFit = true;
    clearTimeout(this.holdTimer);
    this.holdTimer = window.setTimeout(() => {
      this.holdFit = false;
      for (const t of this.terms.values()) t.report();
    }, ms);
  }

  /** The native folder chooser. `null` means the user cancelled it,
      `undefined` means there is none — the caller needs to tell those apart.
      Opens beside the workspace you added last, since the next project is
      almost always its sibling. */
  async pickDirectory(): Promise<string | null | undefined> {
    const last = this.tree.workspaces.at(-1)?.path;
    const parent = last?.replace(/\/[^/]+\/?$/, '') || undefined;
    return pickDirectory(parent);
  }

  /** ⌘] / ⌘[ — move through the current workspace's tabs. */
  cycleTab(delta: number) {
    const ws = this.activeWs;
    // Only what is on the strip: cycling onto a shelved tab would move you
    // somewhere with nothing to show.
    const tabs = this.liveTabs;
    if (!ws || tabs.length < 2) return;
    const i = tabs.findIndex((t) => t.id === this.activeTab?.id);
    const next = tabs[(i + delta + tabs.length) % tabs.length];
    this.activate(ws.id, next.id);
  }

  // ---- derived views ----

  get activeWs() {
    const id = this.localWs ?? this.tree.active_ws;
    return this.tree.workspaces.find((w) => w.id === id) ?? this.tree.workspaces[0];
  }

  /** The tabs on the strip. Shelved ones still exist and still run; they have
      simply given up their place, so everything that walks the strip — the tab
      bar, ⌘]/⌘[, the duplicate-name numbering — goes through here. */
  get liveTabs() {
    return this.activeWs?.tabs.filter((t) => t.shelf === null) ?? [];
  }

  /** The tabs on one shelf. */
  shelved(shelf: Shelf) {
    return this.activeWs?.tabs.filter((t) => t.shelf === shelf) ?? [];
  }

  get activeTab() {
    const ws = this.activeWs;
    if (!ws) return undefined;
    const id = this.localTab ?? this.tree.active_tab;
    const live = this.liveTabs;
    const found = live.find((t) => t.id === id);
    if (found) return found;
    // Normally the strip's first tab. But a viewer whose whole share is a tab
    // the owner then filed would have an empty strip and see nothing at all —
    // filing is not a permission, and it was never meant to take the terminal
    // away from them. So fall back to anything visible.
    return live[0] ?? ws.tabs[0];
  }

  /** Where a follower has wandered to on its own. The server's active ids are
      one shared view: the owner and any share that may open tabs move it
      with `Activate` and all see the same thing. Every other share follows
      it, but may look elsewhere for a while — here — until the owner next
      moves. */
  private localWs = $state<WsId | null>(null);
  private localTab = $state<TabId | null>(null);
  /** The owner's position as last seen, to tell a move from a title change. */
  private ownerAt = '';

  activate(ws: WsId, tab: TabId | null) {
    if (this.caps.may_open_tab) {
      this.send({ t: 'activate', ws, tab });
      return;
    }
    this.localWs = ws;
    this.localTab =
      tab ??
      this.tree.workspaces
        .find((w) => w.id === ws)
        ?.tabs.find((t) => t.shelf === null)?.id ??
      null;
    // A viewer's navigation produces no tree frame, so reconcile never runs and
    // would leave the keyboard on the pane they just navigated away from. They
    // cannot type, but focus is also what makes PageUp scroll the scrollback.
    this.focused = this.visiblePanes()[0]?.id ?? null;
    this.applyFocus();
  }

  /** True when this is the owner looking at an app with no workspaces — a
      fresh install, or one whose last workspace was just closed. The daemon no
      longer opens a folder on our behalf, so the UI must invite the owner to
      choose one. Viewers never see this: a share with nothing in it is the
      owner's problem to fix, not theirs. */
  get needsWorkspacePrompt(): boolean {
    return this.caps.host && this.connected && this.tree.workspaces.length === 0;
  }

  visiblePanes(): PaneView[] {
    return this.activeTab?.panes ?? [];
  }

  pane(id: PaneId): PaneView | undefined {
    for (const ws of this.tree.workspaces) {
      for (const tab of ws.tabs) {
        const p = tab.panes.find((p) => p.id === id);
        if (p) return p;
      }
    }
    return undefined;
  }

  /** Rolled-up phase counts, for the sidebar footer. */
  get statusCounts() {
    const counts = {
      never_ran: 0,
      running: 0,
      needs_input: 0,
      idle: 0,
      success: 0,
      failed: 0,
    };
    for (const ws of this.tree.workspaces) {
      for (const tab of ws.tabs) {
        for (const p of tab.panes) counts[p.status.phase]++;
      }
    }
    return counts;
  }
}

/** The desktop shell exposes a real folder chooser; a browser has to make do.
 *
 *  Three outcomes, and callers depend on telling them apart:
 *    string    — a folder was chosen
 *    null      — the chooser was cancelled, so do nothing
 *    undefined — there is no chooser, so fall back to typing a path
 *  Collapsing cancel into "no chooser" would pop the typed-path dialog at the
 *  moment the user said no. */
async function pickDirectory(startAt?: string): Promise<string | null | undefined> {
  const shell = (window as any).__BEEBOX__;
  if (typeof shell?.pickDirectory !== 'function') return undefined;

  try {
    const picked = await shell.pickDirectory({
      title: 'Open workspace',
      defaultPath: startAt,
    });
    return typeof picked === 'string' ? picked : null;
  } catch {
    // The bridge is there but broke; typing a path still works.
    return undefined;
  }
}

export const store = new Store();

