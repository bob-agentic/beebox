// Application state. Mirrors the server's TreeView and routes output to the
// terminals, which the Pane components own — a Terminal is bound to the element
// it was opened on, so tying its lifetime to that element is what keeps a split
// from leaving one drawing off-screen.

import type { Terminal } from '@xterm/xterm';
import { markRead, pruneRead } from './agent-status';
import { Conn } from './conn';
import { settings } from './settings.svelte';
import { stripPartialLineMarkers } from './partial-line';
import type {
  AgentSettings,
  AgentStatusView,
  Caps,
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

interface Attached {
  term: Terminal;
  /** Re-fits and reports the viewport. Owned by the component. `force` sends
      the size even when it has not changed. */
  report: (force?: boolean) => void;
  /** Output that arrived before this pane mounted. */
  pending: Uint8Array[];
}

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

  /** True when this client may drive the terminal's size — `?phone=1`, which
   *  the Android shell appends and the desktop offers as a checkbox. Only
   *  such a client has any use for a re-fit control: everyone else's size is
   *  the owner's, and asking for it again would change nothing. */
  sizing = $state(false);
  share = $state<{ url: string; hosts: string[] } | null>(null);
  /** Daemon-owned Agents toggles. Owner-only; null until the server sends
      the snapshot. */
  agentSettings = $state<AgentSettings | null>(null);
  codexHooks = $state<string>('missing');
  /** Whether the daemon serves non-loopback clients. Owner-only knob. */
  webExposed = $state(false);
  private wsBase = '';
  /** Bumped whenever this browser marks a completion read. localStorage is
      not reactive, so aggregate dots (tab, workspace) depend on this to
      re-derive their unread flag. */
  readRev = $state(0);

  /** The one write path for read-state, so every dot re-derives together.
      Only bumps when something changed — an unconditional bump inside a
      $effect that also reads readRev would loop forever. */
  markStatusRead(pane: PaneId, view: AgentStatusView) {
    if (markRead(pane, view)) this.readRev++;
  }

  /** Live terminals by pane id. Registered by the Pane components. */
  private terms = new Map<PaneId, Attached>();
  /** Output for panes that have not mounted yet. */
  private buffered = new Map<PaneId, Uint8Array[]>();
  /** pty -> pane, so output frames can be routed without a tree lookup. */
  private ptyToPane = new Map<number, PaneId>();
  private conn: Conn | null = null;

  constructor() {
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
    // Tell the daemon how much this browser can hold, so the replay it sends
    // on connect matches — otherwise it guesses, and either sends more than
    // will fit (parsed and dropped) or less than it could (a half-empty
    // buffer). Read once at connect: it is what the first replay is sized to.
    const replay = settings.current.scrollback;
    // Sizing, for a screen the terminal was not laid out for. A phone shown a
    // 175-column terminal on a 44-column screen gets text overlapping itself —
    // it has to be able to resize the terminal to be readable at all. The cost
    // is that the owner's window resizes with it, so it is not assumed.
    //
    // The native shell injects this before the bundle runs, which is the whole
    // reason it is a marker and not a query parameter: a client that knows
    // what it is should say so once, rather than have every share link carry
    // the answer around and leak it to whoever the link is forwarded to. The
    // desktop checkbox still works, and still travels in the URL, because
    // there the answer really is per-link.
    const native = (window as any).__BEEBOX_APP__ !== undefined;
    this.sizing =
      native || new URLSearchParams(location.search).get('phone') === '1';
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
      `${proto}://${host}/ws${q}${q ? '&' : '?'}replay=${replay}` +
      (sizing ? '&sizing=true' : '') +
      (guess ? `&cols=${guess.cols}&rows=${guess.rows}` : '');

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
      (up) => (this.connected = up),
    );
  }

  send(msg: In) {
    this.conn?.send(msg);
  }

  private handle(msg: Out) {
    switch (msg.t) {
      case 'tree': {
        this.tree = msg.tree;
        // Anything clicked before the first frame arrived was handled as a
        // viewer would handle it, because `host` starts false — and the local
        // override that leaves behind outranks the server's own active ids
        // forever after, so a new workspace would open behind the old one.
        if (msg.caps.host && !this.caps.host) {
          this.localWs = null;
          this.localTab = null;
        }
        this.caps = msg.caps;
        this.reconcile();
        break;
      }
      case 'output': {
        const pane = this.ptyToPane.get(msg.pty);
        if (pane !== undefined) this.write(pane, msg.data);
        break;
      }
      case 'resync': {
        const pane = this.ptyToPane.get(msg.pty);
        if (pane === undefined) break;
        // Reset, re-establish terminal modes, then replay. Without the mode
        // prefix the client would send the wrong bytes for arrow keys and
        // pastes — see ARCHITECTURE.md §5a.
        this.terms.get(pane)?.term.reset();
        this.write(pane, msg.modes);
        this.write(pane, stripPartialLineMarkers(msg.data));
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
      case 'agent_settings': {
        this.agentSettings = msg.settings;
        this.codexHooks = msg.codex_hooks;
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
      case 'peers':
        this.peers = msg.peers;
        break;
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
    }
  }

  /** Refreshes pty routing and keeps the focus on a pane that still exists. */
  private reconcile() {
    const live = new Set<PaneId>();
    this.ptyToPane.clear();

    for (const ws of this.tree.workspaces) {
      for (const tab of ws.tabs) {
        for (const p of tab.panes) {
          live.add(p.id);
          if (p.pty !== null) this.ptyToPane.set(p.pty, p.id);
        }
      }
    }
    for (const id of this.buffered.keys()) {
      if (!live.has(id)) this.buffered.delete(id);
    }
    // Read-state entries for deleted panes go with them.
    pruneRead(live);

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

  /** Called by a Pane once its Terminal is open. */
  register(pane: PaneId, term: Terminal, report: () => void) {
    this.terms.set(pane, { term, report, pending: [] });
    // Anything that arrived before the element existed.
    for (const chunk of this.buffered.get(pane) ?? []) term.write(chunk);
    this.buffered.delete(pane);
    // The tree frame that chose this pane may have arrived before the element
    // did, in which case applyFocus found nothing to focus. This is the other
    // half of that race — without it the first pane after a cold start still
    // needs a click.
    if (this.focused === pane) this.applyFocus();
  }

  unregister(pane: PaneId) {
    this.terms.delete(pane);
  }

  private write(pane: PaneId, data: Uint8Array) {
    const t = this.terms.get(pane);
    if (t) {
      t.term.write(data);
      return;
    }
    // Not mounted yet: hold it so first paint is not blank. Bounded, because
    // a pane in a background tab may never mount.
    const buf = this.buffered.get(pane) ?? [];
    if (buf.length < 256) buf.push(data);
    this.buffered.set(pane, buf);
  }

  /** Bumped on every keystroke sent to a terminal.
   *
   *  Typing is the clearest statement that this tab is the one you are working
   *  in — so the tab strip watches this to scroll back to it. Having scrolled
   *  away to look at another tab and then started typing, the tab you are
   *  actually in is the one that should be on screen. A counter rather than a
   *  flag: the point is that it *changed*, not what it holds. */
  typedRev = $state(0);

  /** Called by a pane when the user types into it. */
  noteTyping() {
    this.typedRev++;
  }

  focus(pane: PaneId) {
    this.focused = pane;
    this.applyFocus();
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

  /** Asks every mounted pane to re-measure and say so, even if the numbers
   *  come out the same. Bound to the re-fit button, which only a client that
   *  drives its own size gets to see. */
  refit() {
    for (const t of this.terms.values()) t.report(true);
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

  /** Viewer-local navigation. `Activate` is owner-only on the server — the
      server's active ids are the owner's view, and a shared client changing
      them would flip the owner's screen. So the owner sends Activate; a
      viewer just remembers its own selection here. */
  private localWs = $state<WsId | null>(null);
  private localTab = $state<TabId | null>(null);

  activate(ws: WsId, tab: TabId | null) {
    if (this.caps.host) {
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

