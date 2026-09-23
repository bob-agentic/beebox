// Mirror of core/src/proto.rs. Kept honest by the round-trip tests on the
// Rust side; the tag strings here must match its serde attributes exactly.

export type PaneId = number;
export type PtyId = number;
export type WsId = number;
export type TabId = number;
export type SessionId = number;

export type AgentKind = 'claude' | 'opencode' | 'codex';

/** Six hook-driven states. `failed` means a structured tool error inside the
    last turn — never the process exit code. */
export type AgentPhase =
  | 'never_ran'
  | 'running'
  | 'needs_input'
  | 'idle'
  | 'success'
  | 'failed';

export interface AgentStatusView {
  phase: AgentPhase;
  agent: AgentKind | null;
  /** Bumped on every accepted event; local read-state keys on it. */
  revision: number;
  at_ms: number;
  started_at_ms: number | null;
  tool_detail: string | null;
  summary: string | null;
}

/** Daemon-owned Agents toggles. All default OFF; owner-only. */
export interface AgentSettings {
  status_claude: boolean;
  status_opencode: boolean;
  status_codex: boolean;
  resume_claude: boolean;
  resume_opencode: boolean;
  resume_codex: boolean;
}

export type AgentSetting = 'status' | 'resume';

/** Codex hooks readiness, probed by the daemon at startup. */
export type CodexHooksState = 'stable' | 'legacy' | 'missing';

export type Dir = 'vertical' | 'horizontal';

export interface GitInfo {
  branch: string;
  added: number;
  modified: number;
}

export type Node =
  | { kind: 'leaf'; pane: PaneId }
  | { kind: 'split'; dir: Dir; children: Node[]; sizes: number[] };

export interface PaneView {
  id: PaneId;
  pty: PtyId | null;
  title: string;
  agent: AgentKind | null;
  status: AgentStatusView;
  session_title: string | null;
  cwd: string;
  git: GitInfo | null;
  cols: number;
  rows: number;
}

/** The two shelves a tab can be filed on, mirroring `proto.rs`. */
export type Shelf = 'archive' | 'later';

export interface TabView {
  id: TabId;
  title: string;
  /** Set aside: still running, just not on the strip. */
  /** Where this tab is filed, or null for one on the strip. Filing only —
      a shelved tab runs, is shared, and carries a status dot exactly as it
      did on the strip. */
  shelf: Shelf | null;
  layout: Node;
  panes: PaneView[];
}

export interface WorkspaceView {
  id: WsId;
  name: string;
  path: string;
  branch: string;
  tabs: TabView[];
}

export interface TreeView {
  workspaces: WorkspaceView[];
  active_ws: WsId | null;
  active_tab: TabId | null;
}

export interface Caps {
  writable: boolean;
  /** May restructure the tree and set pane size. Owner only. */
  /** Holds the owner key, and so may change anything. Never a share link,
      however wide its scope: a link lets someone type into terminals someone
      else owns, and nothing more. */
  host: boolean;
  /** May open a tab — a writable whole-machine share, or the owner. The one
      structural thing a link can do, because the new tab lands where whoever
      asked for it can see it. */
  may_open_tab: boolean;
  show_sidebar: boolean;
  show_tabs: boolean;
}

export interface Peer {
  session: SessionId;
  /** This very connection; it is not offered a way to kick itself. */
  is_you: boolean;
  /** Self-declared at pairing; there is no account system. */
  label: string;
  device: string;
  addr: string;
  scope: string;
  writable: boolean;
  since_secs: number;
}

export type CloseReason = 'revoked' | 'kicked' | 'scope_gone';

export type Out =
  | { t: 'tree'; tree: TreeView; caps: Caps }
  | { t: 'output'; pty: PtyId; data: Uint8Array }
  | { t: 'resync'; pty: PtyId; modes: Uint8Array; data: Uint8Array; through: number }
  | { t: 'status'; pane: PaneId; status: AgentStatusView }
  | { t: 'session_title'; pane: PaneId; text: string }
  | { t: 'agent_settings'; settings: AgentSettings; codex_hooks: string }
  | { t: 'web_server'; exposed: boolean }
  | { t: 'title'; pane: PaneId; text: string }
  | { t: 'cwd'; pane: PaneId; path: string; git: GitInfo | null }
  | { t: 'size'; pane: PaneId; cols: number; rows: number }
  | { t: 'exited'; pane: PaneId; code: number }
  | { t: 'peers'; peers: Peer[] }
  | { t: 'grant'; url: string; pair_code: string | null; hosts: string[] }
  | { t: 'pong' }
  | { t: 'closed'; reason: CloseReason };

export type GrantScope =
  | { kind: 'all' }
  | { kind: 'workspace'; ws: WsId }
  | { kind: 'tab'; tab: TabId }
  | { kind: 'pane'; pane: PaneId };

export type In =
  | { t: 'input'; pane: PaneId; data: Uint8Array }
  | { t: 'viewport'; pane: PaneId; cols: number; rows: number }
  | { t: 'ping' }
  | { t: 'split'; pane: PaneId; dir: Dir }
  | { t: 'set_sizes'; tab: TabId; path: number[]; sizes: number[] }
  | { t: 'close_pane'; pane: PaneId }
  | { t: 'respawn'; pane: PaneId }
  | { t: 'open_tab'; ws: WsId }
  | { t: 'close_tab'; tab: TabId }
  | { t: 'activate'; ws: WsId; tab: TabId | null }
  | { t: 'open_workspace'; path: string }
  | { t: 'close_workspace'; ws: WsId }
  | { t: 'rename_workspace'; ws: WsId; name: string }
  | { t: 'rename_tab'; tab: TabId; title: string }
  | { t: 'shelve_tab'; tab: TabId; shelf: Shelf | null }
  | { t: 'reorder_workspaces'; order: WsId[] }
  | { t: 'reorder_tabs'; ws: WsId; order: TabId[] }
  | { t: 'create_grant'; scope: GrantScope; writable: boolean }
  | { t: 'set_agent_setting'; agent: AgentKind; setting: AgentSetting; on: boolean }
  | { t: 'reset_agent_settings' }
  | { t: 'set_web_server'; exposed: boolean }
  | { t: 'kick'; session: SessionId }
  | { t: 'kick_all' };

/** Leaves of a layout tree, in visual order. */
export function leaves(node: Node): PaneId[] {
  return node.kind === 'leaf' ? [node.pane] : node.children.flatMap(leaves);
}
