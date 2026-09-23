//! Wire protocol. The single source of truth for what crosses the WebSocket.
//!
//! Encoded as MessagePack. Every byte payload is tagged `serde_bytes`: serde's
//! default `Vec<u8>` encoding is an array of integers, measured at +100% on a
//! 4KB frame — worse than the JSON+base64 this format was chosen to avoid.

use serde::{Deserialize, Serialize};

/// Stable identity of a pane. Survives the process dying and being re-run, so
/// layout and share grants reference this, never `PtyId`.
pub type PaneId = u64;

/// Identity of one spawned process. Ephemeral: re-running a pane yields a new
/// one. Only output and input reference it.
pub type PtyId = u64;

pub type WsId = u64;
pub type TabId = u64;
pub type SessionId = u64;

/// Ids are monotonic and never reused. A recycled id would let a stale grant
/// re-authorise a different terminal.
///
/// Which agent CLI a pane is running, if any. This is the protocol enum;
/// display strings ("CC", "CX", "OC", "SH") are a UI concern and never cross
/// the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Opencode,
    Codex,
}

/// Six agent states, driven exclusively by structured hooks — never by
/// matching terminal output, and never by PTY exit codes. `Failed` means a
/// structured tool error occurred inside the last turn; a process exiting is
/// `Out::Exited` and unrelated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPhase {
    NeverRan,
    Running,
    NeedsInput,
    Idle,
    Success,
    Failed,
}

/// Everything a client needs to draw one pane's agent status: the dot, the
/// tooltip, and per-client read tracking.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStatusView {
    pub phase: AgentPhase,
    /// Which agent produced this state. `None` until the first hook arrives.
    pub agent: Option<AgentKind>,
    /// Bumped on every accepted event. Clients key their local read-state on
    /// it — the server never stores who has seen what.
    pub revision: u64,
    /// When the event that produced this state was captured by the adapter.
    /// The store rejects anything older, so out-of-order hook deliveries can
    /// never roll `running` back over `success`.
    pub at_ms: i64,
    /// Start of the current (or last) turn, for "Running for 18s" and
    /// "turn finished · 1m12s".
    pub started_at_ms: Option<i64>,
    /// One sanitized line about the tool in flight, e.g. "Edit src/auth.rs".
    pub tool_detail: Option<String>,
    /// Completion summary from the agent's own transcript — never from
    /// terminal output.
    pub summary: Option<String>,
}

/// A hook event after normalization. Adapters forward raw agent payloads; the
/// daemon reduces them to this, in exactly one place (`hooks.rs`), and the
/// state machine (`agent.rs`) consumes nothing else.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentEvent {
    pub agent: AgentKind,
    pub kind: AgentEventKind,
    /// Captured by the adapter when the agent emitted the event.
    pub at_ms: i64,
    /// The agent's own session id, already whitelist-validated.
    pub session_id: Option<String>,
    /// A session title, when the event carries one. Already sanitized.
    pub session_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEventKind {
    SessionStart,
    PromptSubmit,
    PreTool { detail: Option<String> },
    PostTool { failed: bool },
    NeedsInput,
    TurnEnd { summary: Option<String> },
    SessionEnd,
}

/// Daemon-owned agent settings: six independent toggles, all defaulting to
/// OFF (matching mux0). `status` gates whether hook events drive the dots;
/// `resume` gates whether session ids are persisted and replayed on launch.
/// These are owner-level, not browser-local — Appearance stays in
/// localStorage, this crosses the wire.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSettings {
    pub status_claude: bool,
    pub status_opencode: bool,
    pub status_codex: bool,
    pub resume_claude: bool,
    pub resume_opencode: bool,
    pub resume_codex: bool,
}

impl AgentSettings {
    pub fn status(&self, agent: AgentKind) -> bool {
        match agent {
            AgentKind::Claude => self.status_claude,
            AgentKind::Opencode => self.status_opencode,
            AgentKind::Codex => self.status_codex,
        }
    }
    pub fn resume(&self, agent: AgentKind) -> bool {
        match agent {
            AgentKind::Claude => self.resume_claude,
            AgentKind::Opencode => self.resume_opencode,
            AgentKind::Codex => self.resume_codex,
        }
    }
    pub fn set(&mut self, agent: AgentKind, setting: AgentSetting, on: bool) {
        let slot = match (setting, agent) {
            (AgentSetting::Status, AgentKind::Claude) => &mut self.status_claude,
            (AgentSetting::Status, AgentKind::Opencode) => &mut self.status_opencode,
            (AgentSetting::Status, AgentKind::Codex) => &mut self.status_codex,
            (AgentSetting::Resume, AgentKind::Claude) => &mut self.resume_claude,
            (AgentSetting::Resume, AgentKind::Opencode) => &mut self.resume_opencode,
            (AgentSetting::Resume, AgentKind::Codex) => &mut self.resume_codex,
        };
        *slot = on;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSetting {
    Status,
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    /// ⌘D — panes side by side.
    Vertical,
    /// ⇧⌘D — panes stacked.
    Horizontal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitInfo {
    pub branch: String,
    pub added: u32,
    pub modified: u32,
}

/// Layout geometry. Recursive by design even though the UI caps nesting at two
/// levels — lifting the cap later must not require a schema migration.
///
/// Leaves hold `PaneId`: a pane keeps its place in the tree across re-runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Node {
    Leaf {
        pane: PaneId,
    },
    Split {
        dir: Dir,
        children: Vec<Node>,
        /// Fractions, not pixels, so a layout survives a different window size.
        sizes: Vec<f32>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneView {
    pub id: PaneId,
    /// `None` once the process has exited — the pane stays on screen so it can
    /// be re-run, which is why this is separate from `PaneId`.
    pub pty: Option<PtyId>,
    /// Follows OSC 0/2, so it tracks the running command rather than the
    /// spawn argv.
    pub title: String,
    /// Which agent CLI this pane runs, once one has been seen. The UI maps it
    /// to its badge (claude→CC, codex→CX, opencode→OC, none→SH).
    pub agent: Option<AgentKind>,
    pub status: AgentStatusView,
    /// The agent session's own title, for auto tab naming. Distinct from
    /// `title` (OSC) — this comes only from the agent's structured metadata.
    pub session_title: Option<String>,
    pub cwd: String,
    pub git: Option<GitInfo>,
    /// Authoritative size, owned by the server. See ARCHITECTURE.md §3.
    pub cols: u16,
    pub rows: u16,
}

/// Where a tab is filed, when it is not on the strip.
///
/// Filing only, not a mode: a shelved tab runs exactly as it did, is shared
/// exactly as it was, and carries the same status dot. The two shelves exist
/// because a tab you are done with and a tab you have not started are worth
/// telling apart when you come back to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shelf {
    /// Finished with, kept to look back at.
    Archive,
    /// Not started yet.
    Later,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabView {
    pub id: TabId,
    pub title: String,
    /// Off the strip and on a shelf, or `None` for a tab on the strip. The
    /// tab keeps running either way.
    pub shelf: Option<Shelf>,
    /// Authoritative for structure. Pane membership is never derived from
    /// anywhere else.
    pub layout: Node,
    pub panes: Vec<PaneView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceView {
    pub id: WsId,
    pub name: String,
    pub path: String,
    pub branch: String,
    pub tabs: Vec<TabView>,
}

/// The tree as one connection is allowed to see it, already filtered by scope.
/// A Pane-scoped viewer receives exactly one workspace, one tab, one pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeView {
    pub workspaces: Vec<WorkspaceView>,
    /// Which workspace/tab the owner has in front. Viewers follow it so a
    /// demo stays in sync.
    pub active_ws: Option<WsId>,
    pub active_tab: Option<TabId>,
}

/// What this connection may do. The client trims chrome accordingly; the server
/// enforces it regardless of what the client believes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Caps {
    /// May send `Input`.
    pub writable: bool,
    /// Holds the owner key, and so may change anything: splitting, opening
    /// and closing tabs and workspaces, renaming, reordering, filing on
    /// shelves, minting links, configuring the daemon, kicking sessions.
    ///
    /// Never a share link, however wide its scope. A link lets someone type
    /// into terminals someone else owns, and nothing more — handing out a
    /// whole-machine link is not handing over the machine.
    pub host: bool,
    /// May open a tab: a writable share of a workspace or wider. The one
    /// structural thing a link can do, because a new tab lands where whoever
    /// asked for it can see it.
    pub may_open_tab: bool,
    pub show_sidebar: bool,
    pub show_tabs: bool,
}

/// A device paired on a share link. One link, one device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    /// The link's token — what a revoke names.
    pub token: String,
    /// Coarse, from the user agent at pairing: "Chrome / Android".
    pub device: String,
    /// Where it is connected from; `None` while it is offline.
    pub addr: Option<String>,
    pub scope: String,
    pub writable: bool,
    /// Unix seconds.
    pub paired_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    /// The link was revoked; it will not work again.
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Out {
    /// Sent on connect and re-sent, scope-filtered, after any structural
    /// change. Whole-tree resend is correct at this scale (tens of panes) and
    /// keeps two windows on one session in sync without delta plumbing.
    Tree {
        tree: TreeView,
        caps: Caps,
    },

    /// Hot path. One frame carries one PTY's bytes; never batched across PTYs.
    Output {
        pty: PtyId,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },

    /// Replay after a reconnect or a subscriber overflow. `modes` re-establishes
    /// terminal state (alt screen, bracketed paste, application cursor keys)
    /// that a raw byte-ring tail would have lost — see ARCHITECTURE.md §4.
    /// The client must reset its terminal, apply `modes`, then write `data`.
    Resync {
        pty: PtyId,
        #[serde(with = "serde_bytes")]
        modes: Vec<u8>,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
        /// Absolute ring offset this replay ends at, so the subscriber resumes
        /// without a duplicated or dropped seam.
        through: u64,
    },

    /// Live agent-status delta. Carries the whole view, not just the phase,
    /// so a client joining mid-turn still gets the tooltip fields and the
    /// revision it keys read-state on.
    Status {
        pane: PaneId,
        status: AgentStatusView,
    },
    /// Live agent session-title delta, for auto tab naming.
    SessionTitle {
        pane: PaneId,
        text: String,
    },
    /// Daemon-owned agent settings snapshot. Sent to owner connections on
    /// connect and after every change, so two windows stay in step.
    /// `codex_hooks` is probed at startup: `stable` (modern Codex, no badge),
    /// `legacy` (old feature-flag era, show BETA), or `missing` (no codex).
    AgentSettings {
        settings: AgentSettings,
        codex_hooks: String,
    },
    /// Whether non-loopback clients are allowed in. The daemon always serves
    /// loopback (the terminal itself rides on HTTP); this gates everyone
    /// else. Owner-only, toggled from the status bar.
    WebServer {
        exposed: bool,
    },
    Title {
        pane: PaneId,
        text: String,
    },
    Cwd {
        pane: PaneId,
        path: String,
        git: Option<GitInfo>,
    },
    /// Authoritative size, pushed after the server resolves it. Clients render
    /// this in the pane header and letterbox to fit.
    Size {
        pane: PaneId,
        cols: u16,
        rows: u16,
    },
    /// Process exited; the pane remains so it can be re-run.
    Exited {
        pane: PaneId,
        code: i32,
    },

    Peers {
        peers: Vec<Peer>,
    },
    /// Answer to `CreateGrant`. `hosts` lists every address the daemon can be
    /// reached on (loopback first, then each interface), like a dev server's
    /// startup banner — the client renders one full URL per host.
    Grant {
        url: String,
        hosts: Vec<String>,
    },
    Pong,
    Closed {
        reason: CloseReason,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum In {
    // ---- available to any writable connection ----
    /// Keystrokes. Names its pane explicitly: there is no implicit "current
    /// pane" on the server, so two viewers can focus different panes without
    /// fighting.
    Input {
        pane: PaneId,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    /// Advisory — this client's viewport. The server decides the PTY's actual
    /// size (owner wins), so a phone joining cannot reflow the owner's pane.
    Viewport {
        pane: PaneId,
        cols: u16,
        rows: u16,
    },
    Ping,

    // ---- owner only; rejected otherwise ----
    Split {
        pane: PaneId,
        dir: Dir,
    },
    /// Gutter drag. Fractions, summing to 1.
    SetSizes {
        tab: TabId,
        path: Vec<u32>,
        sizes: Vec<f32>,
    },
    ClosePane {
        pane: PaneId,
    },
    /// Re-run a pane whose process exited.
    Respawn {
        pane: PaneId,
    },
    OpenTab {
        ws: WsId,
    },
    CloseTab {
        tab: TabId,
    },
    Activate {
        ws: WsId,
        tab: Option<TabId>,
    },
    OpenWorkspace {
        path: String,
    },
    CloseWorkspace {
        ws: WsId,
    },
    /// Double-click a title to rename, as mux0 does. Empty clears it back to
    /// the derived default.
    RenameWorkspace {
        ws: WsId,
        name: String,
    },
    RenameTab {
        tab: TabId,
        title: String,
    },

    /// Files a tab on a shelf, or `None` to bring it back to the strip. The
    /// tab and its processes survive either way — only where it is filed
    /// changes.
    ShelveTab {
        tab: TabId,
        shelf: Option<Shelf>,
    },
    /// Drag to reorder. The client sends the full order rather than a
    /// from/to pair, so a dropped frame cannot leave the two sides disagreeing.
    ReorderWorkspaces {
        order: Vec<WsId>,
    },
    ReorderTabs {
        ws: WsId,
        order: Vec<TabId>,
    },
    CreateGrant {
        scope: GrantScope,
        writable: bool,
    },
    /// Owner-only. Flips one of the six agent toggles.
    SetAgentSetting {
        agent: AgentKind,
        setting: AgentSetting,
        on: bool,
    },
    /// Owner-only. Turns all six toggles off and clears every stored resume
    /// session id.
    ResetAgentSettings,
    /// Owner-only. Opens (or closes) the port to non-loopback clients.
    SetWebServer {
        exposed: bool,
    },
    /// Deletes a link, disconnecting the device paired on it for good.
    Revoke {
        token: String,
    },
    /// Every link at once.
    RevokeAll,
}

/// Wire form of a share scope. Mirrors `share::Scope` but stays in the protocol
/// so the client can request a grant.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GrantScope {
    All,
    Workspace { ws: WsId },
    Tab { tab: TabId },
    Pane { pane: PaneId },
}
