// Application state. Mirrors the server's TreeView and routes output to the
// terminals, which the Pane components own — a Terminal is bound to the element
// it was opened on, so tying its lifetime to that element is what keeps a split
// from leaving one drawing off-screen.

import type { Terminal } from '@xterm/xterm';
import { markRead, pruneRead } from './agent-status';
import { Conn } from './conn';
import type {
  AgentSettings,
  AgentStatusView,
  Caps,
  In,
  Out,
  PaneId,
  PaneView,
  Peer,
  TabId,
  TreeView,
  WsId,
} from './proto';

interface Attached {
  term: Terminal;
  /** Re-fits and reports the viewport. Owned by the component. */
  report: () => void;
  /** Output that arrived before this pane mounted. */
  pending: Uint8Array[];
}

const EMPTY_TREE: TreeView = { workspaces: [], active_ws: null, active_tab: null };

class Store {
  tree = $state<TreeView>(EMPTY_TREE);
  caps = $state<Caps>({ writable: false, owner: false, show_sidebar: false, show_tabs: false });
  connected = $state(false);
  /** Why the server closed us, if it did. */
  closedReason = $state<string | null>(null);
  peers = $state<Peer[]>([]);
  /** Focused pane. Local to this browser: the server has no "current pane". */
  focused = $state<PaneId | null>(null);
  share = $state<{ url: string; pair_code: string | null; hosts: string[] } | null>(null);
  /** Daemon-owned Agents toggles. Owner-only; null until the server sends
      the snapshot. */
  agentSettings = $state<AgentSettings | null>(null);
  codexHooks = $state<string>('missing');
  /** Whether the daemon serves non-loopback clients. Owner-only knob. */
  webExposed = $state(false);
  /** Set when this share link demands a pairing code before connecting.
      The App renders the code prompt instead of the terminal. */
  needsPairing = $state(false);
  pairError = $state(false);
  private shareToken: string | null = null;
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
    this.shareToken = m ? m[1] : null;
    const q = this.shareToken
      ? `?token=${encodeURIComponent(this.shareToken)}`
      : key
        ? `?key=${encodeURIComponent(key)}`
        : '';
    this.wsBase = `${proto}://${host}/ws${q}`;

    if (this.shareToken) {
      // A paired grant refuses the socket until the code is presented, and a
      // WebSocket carries no HTTP status back — so ask first, over plain
      // HTTP, and show the prompt instead of a silent dead reconnect loop.
      void fetch(`/pair/${encodeURIComponent(this.shareToken)}`)
        .then((r) => (r.ok ? r.json() : { pairing: false }))
        .then((s: { pairing: boolean }) => {
          if (s.pairing) this.needsPairing = true;
          else this.connect();
        })
        .catch(() => this.connect());
    } else {
      this.connect();
    }
  }

  private connect(pair?: string) {
    this.conn?.dispose();
    const url = pair ? `${this.wsBase}&pair=${encodeURIComponent(pair)}` : this.wsBase;
    this.conn = new Conn(
      url,
      (msg) => this.handle(msg),
      (up) => (this.connected = up),
    );
  }

  /** Called by the pairing prompt. The code is single-use: on success the
      server clears it and the plain link works from then on. */
  submitPairCode(code: string) {
    this.pairError = false;
    const clean = code.trim().toUpperCase();
    if (!clean) return;
    // Probe with a plain fetch first so a wrong code shows an error instead
    // of an opaque socket failure.
    void fetch(`/pair/${encodeURIComponent(this.shareToken!)}`)
      .then(() => {
        this.needsPairing = false;
        this.connect(clean);
        // If the code was wrong the socket dies instantly and pairing is
        // still pending server-side; surface the prompt again with an error.
        setTimeout(() => {
          if (!this.connected) {
            this.needsPairing = true;
            this.pairError = true;
          }
        }, 1500);
      });
  }

  send(msg: In) {
    this.conn?.send(msg);
  }

  private handle(msg: Out) {
    switch (msg.t) {
      case 'tree': {
        this.tree = msg.tree;
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
        this.write(pane, msg.data);
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
        this.share = { url: msg.url, pair_code: msg.pair_code, hosts: msg.hosts };
        break;
      case 'closed':
        this.connected = false;
        this.closedReason = msg.reason;
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

  focus(pane: PaneId) {
    this.focused = pane;
    this.applyFocus();
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

  /** Asks every mounted pane to re-measure. */
  refit() {
    for (const t of this.terms.values()) t.report();
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
    if (!ws || ws.tabs.length < 2) return;
    const i = ws.tabs.findIndex((t) => t.id === this.activeTab?.id);
    const next = ws.tabs[(i + delta + ws.tabs.length) % ws.tabs.length];
    this.activate(ws.id, next.id);
  }

  // ---- derived views ----

  get activeWs() {
    const id = this.localWs ?? this.tree.active_ws;
    return this.tree.workspaces.find((w) => w.id === id) ?? this.tree.workspaces[0];
  }

  get activeTab() {
    const ws = this.activeWs;
    if (!ws) return undefined;
    const id = this.localTab ?? this.tree.active_tab;
    return ws.tabs.find((t) => t.id === id) ?? ws.tabs[0];
  }

  /** Viewer-local navigation. `Activate` is owner-only on the server — the
      server's active ids are the owner's view, and a shared client changing
      them would flip the owner's screen. So the owner sends Activate; a
      viewer just remembers its own selection here. */
  private localWs = $state<WsId | null>(null);
  private localTab = $state<TabId | null>(null);

  activate(ws: WsId, tab: TabId | null) {
    if (this.caps.owner) {
      this.send({ t: 'activate', ws, tab });
      return;
    }
    this.localWs = ws;
    this.localTab =
      tab ?? this.tree.workspaces.find((w) => w.id === ws)?.tabs[0]?.id ?? null;
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
    return this.caps.owner && this.connected && this.tree.workspaces.length === 0;
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

