//! Shared application state and the operations the WebSocket layer performs
//! on it.
//!
//! Keeping the mutations here rather than in `http.rs` means every one of them
//! is unit-testable without a socket, and the authorisation check sits next to
//! the thing it guards.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::{broadcast, Mutex};

use crate::proto::{
    AgentEvent, AgentKind, AgentSetting, AgentSettings, AgentStatusView, Caps, In, Out, PaneId,
    CloseReason, Peer, PtyId, SessionId, TabId, TreeView, WsId,
};
use crate::pty::{Registry, Spawn};
use crate::session::SessionTree;
use crate::share::{visible_panes, Grant, Scope};
use crate::store::Store;

fn random_key() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    (0..4)
        .map(|_| format!("{:08x}", rng.random::<u32>()))
        .collect()
}

/// Broadcast to every connection when the tree changes. Each connection
/// re-renders its own scope-filtered view; whole-tree resend keeps two windows
/// on one session in sync without delta plumbing.
#[derive(Debug, Clone, Copy)]
pub struct TreeChanged;

/// Live agent deltas. Separate from `TreeChanged` because a busy agent fires
/// several tool events per second, and re-sending the whole tree for each
/// would drown the hot path for no reason.
#[derive(Debug, Clone)]
pub enum AgentDelta {
    Status { pane: PaneId, status: AgentStatusView },
    SessionTitle { pane: PaneId, text: String },
    /// Settings changed. Owner connections re-send the snapshot.
    Settings { settings: AgentSettings },
    /// The pane's shell moved somewhere else (or its git state changed).
    Cwd { pane: PaneId, path: String, git: Option<crate::proto::GitInfo> },
}

/// The token the owner's own grant carries. Never a row in `grants`.
const OWNER_TOKEN: &str = "owner";

/// One live WebSocket. What a revoke has to reach — deleting the grant row is
/// not enough on its own, because the socket it authorised is already open.
#[derive(Debug)]
pub struct Conn {
    /// The grant it came in on; `owner` for the owner's own windows.
    token: String,
    addr: String,
    /// Closes the socket, saying why if it should not come back.
    close: Option<tokio::sync::oneshot::Sender<Option<CloseReason>>>,
}

pub struct App {
    pub tree: Mutex<SessionTree>,
    pub ptys: Arc<Registry>,
    pub store: Mutex<Store>,
    /// Per-pane size the owner has asked for. Viewers are advisory only, so
    /// their viewports never land here.
    owner_size: Mutex<HashMap<PaneId, (u16, u16)>>,
    /// Per-pane hook secret. Kept in memory only: it is regenerated on
    /// restart, and rotated whenever a pane spawns, so events from a previous
    /// process can never impersonate the new one.
    hook_secrets: Mutex<HashMap<PaneId, String>>,
    changed: broadcast::Sender<TreeChanged>,
    /// Agent status/title deltas, fanned out to every socket.
    agent_tx: broadcast::Sender<AgentDelta>,
    /// Where the hook endpoint is reachable from processes on this machine.
    /// Set once at startup, injected into every PTY's environment.
    hook_port: std::sync::atomic::AtomicU16,
    /// Adapter assets on disk (hook sender, Claude settings overlay, zsh
    /// shim). `None` when installation failed — panes then spawn without
    /// agent wiring, which is the fail-open the handover requires.
    adapter_assets: Option<crate::agent_adapters::AdapterAssets>,
    /// The six Agents toggles, mirroring the settings table. All default OFF.
    agent_settings: Mutex<AgentSettings>,
    /// Codex hooks readiness, probed once at startup: "stable", "legacy" or
    /// "missing". The UI derives its badge from this instead of a hard-coded
    /// BETA tag — the handover is explicit that stale capability hints are a
    /// bug, not a feature.
    codex_hooks: std::sync::OnceLock<String>,
    /// Whether non-loopback clients are served. The daemon may listen on
    /// 0.0.0.0 so sharing needs no restart, but until the owner opens this,
    /// reaching the port from the network gets nothing. Loopback is always
    /// served — the terminal itself rides on this HTTP server.
    exposed: std::sync::atomic::AtomicBool,
    /// Live connections, keyed by session. Populated on connect, drained on
    /// disconnect — so a paired device can be shown as online or not.
    conns: Mutex<HashMap<SessionId, Conn>>,
    next_session: Mutex<SessionId>,
    pub scrollback_lines: usize,
    pub shell: String,
    /// Proves a connection is the owner. Generated per run and never written
    /// to disk: without it, anything that can reach the port would have a full
    /// terminal on this machine, and the default bind is every interface.
    owner_key: String,
}

impl App {
    pub fn new(store: Store, scrollback_lines: usize) -> Arc<Self> {
        Self::new_inner(store, scrollback_lines, None)
    }

    /// Like `new`, but also installs the agent adapter assets under `home`.
    /// Installation failure is logged and ignored: terminals must work even
    /// when the hook plumbing cannot be written.
    pub fn new_with_adapters(store: Store, scrollback_lines: usize, home: &std::path::Path) -> Arc<Self> {
        let assets = match crate::agent_adapters::install(home) {
            Ok(a) => Some(a),
            Err(e) => {
                tracing::warn!("agent adapters unavailable: {e}");
                None
            }
        };
        Self::new_inner(store, scrollback_lines, assets)
    }

    fn new_inner(
        store: Store,
        scrollback_lines: usize,
        adapter_assets: Option<crate::agent_adapters::AdapterAssets>,
    ) -> Arc<Self> {
        let tree = store.load_tree().expect("read state.db");
        let agent_settings = store.load_agent_settings().expect("read state.db");
        let (changed, _) = broadcast::channel(64);
        let (agent_tx, _) = broadcast::channel(256);
        Arc::new(Self {
            tree: Mutex::new(tree),
            ptys: Registry::new(),
            store: Mutex::new(store),
            owner_size: Mutex::new(HashMap::new()),
            hook_secrets: Mutex::new(HashMap::new()),
            changed,
            agent_tx,
            hook_port: std::sync::atomic::AtomicU16::new(0),
            adapter_assets,
            agent_settings: Mutex::new(agent_settings),
            codex_hooks: std::sync::OnceLock::new(),
            exposed: std::sync::atomic::AtomicBool::new(false),
            conns: Mutex::new(HashMap::new()),
            next_session: Mutex::new(0),
            scrollback_lines,
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into()),
            owner_key: random_key(),
        })
    }

    pub fn subscribe_tree(&self) -> broadcast::Receiver<TreeChanged> {
        self.changed.subscribe()
    }

    /// Persists and announces. Called after every structural change so no
    /// caller can forget half of it.
    async fn commit(&self) {
        let tree = self.tree.lock().await;
        self.store.lock().await.save_tree(&tree).expect("write state.db");
        drop(tree);
        let _ = self.changed.send(TreeChanged);
    }

    pub async fn view_for(&self, grant: &Grant) -> (TreeView, Caps) {
        let tree = self.tree.lock().await;
        let visible = visible_panes(&grant.scope, &tree);
        let view = tree.to_view(&visible);
        let caps = Caps {
            writable: grant.writable,
            host: grant.host,
            may_open_tab: grant.host || grant.may_open_tab(),
            // Chrome is trimmed to the scope so a viewer is never shown
            // navigation into places it cannot reach.
            show_sidebar: matches!(grant.scope, Scope::All),
            show_tabs: matches!(grant.scope, Scope::All | Scope::Workspace(_)),
        };
        (view, caps)
    }

    pub async fn visible(&self, grant: &Grant) -> std::collections::HashSet<PaneId> {
        let tree = self.tree.lock().await;
        visible_panes(&grant.scope, &tree)
    }

    /// Starts a process for a pane that has none. Idempotent: a pane that is
    /// already running is left alone.
    ///
    /// Deliberately uses the owner's last known viewport when there is one. A
    /// shell draws its prompt as soon as it starts, and a right-aligned prompt
    /// redrawn at a new width leaves zsh's reverse-video `%` partial-line
    /// marker behind — which is what the stray glyphs were.
    pub async fn ensure_running(&self, pane: PaneId) -> Result<PtyId> {
        let hint = self.owner_size.lock().await.get(&pane).copied();
        let mut tree = self.tree.lock().await;
        let p = tree.pane(pane).ok_or_else(|| anyhow!("no pane {pane}"))?;
        if let Some(pty) = p.pty {
            return Ok(pty);
        }
        let (cols, rows) = hint.unwrap_or((p.cols, p.rows));

        // Login and interactive, like every real terminal emulator. Without
        // `-l` the shell skips `.zprofile`, which on macOS is where Homebrew
        // puts its PATH — so a bundled app, whose environment comes from
        // launchd rather than a shell, gave panes no `brew` at all.
        //
        // This was once blamed for zsh's reverse-video `%` marker and reverted.
        // That was a misattribution: zsh erases the marker by padding to an
        // exact column count, so it survives whenever the spawn size and the
        // real viewport disagree — regardless of `-l`. The size is the bug,
        // and it is fixed where the size comes from.
        let cmd = if p.cmd.is_empty() {
            vec![self.shell.clone(), "-l".into(), "-i".into()]
        } else {
            p.cmd.clone()
        };

        // Rotate the hook secret on every spawn: a hook fired by the previous
        // process must not be able to paint status onto the new one.
        let secret = Self::mint_secret();
        self.hook_secrets.lock().await.insert(pane, secret.clone());

        // What the agent adapters need to find their way back here. Loopback
        // by construction — the hook route refuses anything else anyway.
        let mut env = Vec::new();
        let port = self.hook_port.load(std::sync::atomic::Ordering::Relaxed);
        if port != 0 {
            env.push(("BEEBOX_PANE_ID".to_string(), pane.to_string()));
            env.push((
                "BEEBOX_HOOK_URL".to_string(),
                format!("http://127.0.0.1:{port}/hooks/{pane}/{secret}"),
            ));
            // The zsh shim and hook sender only make sense when there is a
            // hook endpoint to talk to.
            if let Some(assets) = &self.adapter_assets {
                env.extend(crate::agent_adapters::pane_env(assets, &self.shell));
            }
        }

        let spec = Spawn {
            pane,
            cmd,
            cwd: p.cwd.clone(),
            cols,
            rows,
            env,
            scrollback_lines: self.scrollback_lines,
        };
        let pty = self.ptys.spawn(spec)?;
        let p = tree.pane_mut(pane).expect("just read");
        p.pty = Some(pty);
        p.cols = cols;
        p.rows = rows;

        // Auto-resume: a pane that ran an agent gets its session back. The
        // command is typed into the interactive shell (rc already loaded, so
        // the wrapper function is what receives it), built from the trusted
        // enum plus the whitelisted id — never from stored shell text.
        // Non-destructive: the ref stays, so quitting without a new prompt
        // resumes the same session again next time.
        //
        // Only for panes without an explicit spawn command; an explicit
        // command keeps priority (existing product semantics: resume > none,
        // explicit cmd already replaced the shell entirely above).
        let resume = if p.cmd.is_empty() {
            match (p.agent, &p.session_ref) {
                (Some(agent), Some(id)) => {
                    crate::agent::resume_command(agent, id).map(|cmd| (agent, cmd))
                }
                _ => None,
            }
        } else {
            None
        };
        drop(tree);

        if let Some((agent, cmd)) = resume {
            if self.agent_settings.lock().await.resume(agent) {
                let ptys = Arc::clone(&self.ptys);
                // Give the shell a beat to reach its prompt: input written
                // during rc execution can be swallowed by prompt frameworks
                // (powerlevel10k instant prompt drains the queue).
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                    let _ = ptys.write(pty, format!("{cmd}\r").as_bytes());
                });
            }
        }
        Ok(pty)
    }

    /// Marks a pane's process as gone. The pane itself stays so it can be
    /// re-run — that is why `PaneId` and `PtyId` are separate.
    ///
    /// Deliberately does not touch agent status: a PTY exit is not a turn
    /// result. The client sees `Out::Exited`; the dot keeps whatever the last
    /// hook said.
    pub async fn mark_exited(&self, pane: PaneId) {
        if let Some(p) = self.tree.lock().await.pane_mut(pane) {
            p.pty = None;
        }
    }

    pub async fn pty_of(&self, pane: PaneId) -> Option<PtyId> {
        self.tree.lock().await.pane(pane)?.pty
    }

    /// Applies a viewport.
    ///
    /// A PTY has one size, so not everyone watching gets to set it — two
    /// windows of different widths would flap it between them, repainting the
    /// whole TUI on every change. Normally that means the owner decides.
    ///
    /// `sizing` lets a share link opt in: a phone is useless as a viewer of a
    /// 175-column terminal, since the text arrives laid out for a screen it
    /// does not have and overlaps itself. Such a link takes the terminal with
    /// it, which is the trade the person sharing agreed to.
    pub async fn set_viewport(
        &self,
        grant: &Grant,
        sizing: bool,
        pane: PaneId,
        cols: u16,
        rows: u16,
    ) -> Option<(u16, u16)> {
        if !grant.host && !sizing {
            return None;
        }
        if cols == 0 || rows == 0 {
            return None;
        }
        self.owner_size.lock().await.insert(pane, (cols, rows));

        // A pane with no process yet still records the size, so when it does
        // start it starts at the right width.
        if let Some(pty) = self.pty_of(pane).await {
            if self.ptys.resize(pty, cols, rows).is_err() {
                return None;
            }
        }
        if let Some(p) = self.tree.lock().await.pane_mut(pane) {
            p.cols = cols;
            p.rows = rows;
        }
        Some((cols, rows))
    }

    /// Handles one structural message. Returns the frames to send back to the
    /// caller; the tree broadcast reaches everyone else.
    pub async fn handle_host(&self, grant: &Grant, msg: In) -> Result<Vec<Out>> {
        // The owner may do all of this. A share may open a tab, and only in a
        // scope that will hold one — see `Grant::may_open_tab`. Nothing else:
        // a link lets someone type into terminals someone else owns.
        let allowed = grant.host
            || (matches!(msg, In::OpenTab { .. }) && grant.may_open_tab());
        if !allowed {
            // Not an error worth telling the client about in detail — a
            // well-behaved client never sends these without `caps.host`.
            return Ok(Vec::new());
        }

        match msg {
            In::Split { pane, dir } => {
                let new = self.tree.lock().await.split(pane, dir)?;
                self.ensure_running(new).await?;
                self.commit().await;
            }
            In::ClosePane { pane } => {
                self.close_pane_inner(pane).await;
            }
            In::Respawn { pane } => {
                self.ensure_running(pane).await?;
                self.commit().await;
            }
            In::OpenTab { ws } => {
                let tab = self.tree.lock().await.open_tab(ws)?;
                let pane = self
                    .tree
                    .lock()
                    .await
                    .first_pane(tab)
                    .ok_or_else(|| anyhow!("tab {tab} has no pane"))?;
                self.ensure_running(pane).await?;
                self.commit().await;
            }
            In::CloseTab { tab } => {
                self.close_tab_inner(tab).await;
                self.commit().await;
            }
            In::Activate { ws, tab } => {
                let mut tree = self.tree.lock().await;
                tree.active_ws = Some(ws);
                match tab {
                    Some(t) => {
                        tree.active_tab = Some(t);
                        // Records where you are, and pushes where you were
                        // behind it — that history is what lets closing a tab
                        // return you to the one you came from. Only if the tab
                        // really belongs here: a foreign id would sit at the
                        // head of the list matching nothing.
                        if let Some(w) = tree.workspaces.iter_mut().find(|w| w.id == ws) {
                            if w.tabs.iter().any(|x| x.id == t) {
                                w.touch_tab(t);
                            }
                        }
                    }
                    // Activating a workspace without naming a tab lands on the
                    // one it was last showing. Keeping the *previous*
                    // workspace's tab id here matched nothing, so the tab bar
                    // lost its highlight entirely; falling back to the first
                    // tab lost your place instead.
                    None => {
                        // `active_tab` already skips shelved tabs and falls
                        // back to the first one on the strip.
                        tree.active_tab = tree
                            .workspaces
                            .iter()
                            .find(|w| w.id == ws)
                            .and_then(|w| w.active_tab());
                    }
                }
                drop(tree);
                let _ = self.changed.send(TreeChanged);
            }
            In::SetSizes { tab, path, sizes } => {
                self.tree.lock().await.set_sizes(tab, &path, &sizes)?;
                self.commit().await;
            }
            In::OpenWorkspace { path } => {
                // An empty path means "just give me a workspace": the user's
                // home directory. Keeps first-run and ⌘N prompt-free.
                let path = if path.trim().is_empty() {
                    std::env::var("HOME").unwrap_or_else(|_| "/".into())
                } else {
                    path
                };
                let base = std::path::Path::new(&path)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());

                // ⌘N opens another workspace on the same folder, so the names
                // would collide. Number the repeats, as a browser does.
                let taken = {
                    let tree = self.tree.lock().await;
                    tree.workspaces
                        .iter()
                        .filter(|w| w.name == base || w.name.starts_with(&format!("{base} ")))
                        .count()
                };
                let name = if taken == 0 {
                    base
                } else {
                    format!("{base} {}", taken + 1)
                };
                let ws = self.tree.lock().await.open_workspace(path, name);
                let tab = self.tree.lock().await.open_tab(ws)?;
                let pane = self.tree.lock().await.first_pane(tab).unwrap();
                self.ensure_running(pane).await?;
                self.commit().await;
            }
            In::RenameWorkspace { ws, name } => {
                if let Some(w) = self.tree.lock().await.workspaces.iter_mut().find(|w| w.id == ws) {
                    // An empty name falls back to the folder, so a cleared
                    // field never leaves a nameless row.
                    w.name = if name.trim().is_empty() {
                        std::path::Path::new(&w.path)
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| w.path.clone())
                    } else {
                        name.trim().to_string()
                    };
                }
                self.commit().await;
            }
            In::RenameTab { tab, title } => {
                if let Some(t) = self.tree.lock().await.tab_mut(tab) {
                    t.title = title.trim().to_string();
                }
                self.commit().await;
            }
            In::ShelveTab { tab, shelf } => {
                // No pty is touched: the point of filing a tab rather than
                // closing it is that whatever is running keeps running.
                self.tree.lock().await.shelve_tab(tab, shelf);
                self.commit().await;
            }
            In::ReorderWorkspaces { order } => {
                let mut tree = self.tree.lock().await;
                // Anything the client did not mention keeps its relative place
                // at the end, so an order computed against a stale tree cannot
                // drop a workspace.
                tree.workspaces.sort_by_key(|w| {
                    order.iter().position(|id| *id == w.id).unwrap_or(usize::MAX)
                });
                drop(tree);
                self.commit().await;
            }
            In::ReorderTabs { ws, order } => {
                let mut tree = self.tree.lock().await;
                if let Some(w) = tree.workspaces.iter_mut().find(|w| w.id == ws) {
                    w.tabs.sort_by_key(|t| {
                        order.iter().position(|id| *id == t.id).unwrap_or(usize::MAX)
                    });
                }
                drop(tree);
                self.commit().await;
            }
            In::CloseWorkspace { ws } => {
                let panes = self.tree.lock().await.close_workspace(ws);
                for pane in panes {
                    if let Some(pty) = self.pty_of(pane).await {
                        self.ptys.kill(pty);
                    }
                }
                self.commit().await;
            }
            In::Revoke { token } => self.revoke(&token).await,
            In::RevokeAll => self.revoke_all().await,
            In::SetAgentSetting { agent, setting, on } => {
                self.set_agent_setting(agent, setting, on).await;
            }
            In::ResetAgentSettings => {
                self.reset_agent_settings().await;
            }
            In::SetWebServer { exposed } => {
                self.set_exposed(exposed).await;
            }
            // Not owner-gated; handled by the caller.
            In::Input { .. } | In::Viewport { .. } | In::Ping | In::CreateGrant { .. } => {}
        }
        Ok(Vec::new())
    }

    /// Watches every pty for its exit, and acts on it.
    ///
    /// One pump for the whole app, not one per connection: the registry's
    /// broadcast reaches every socket, and closing a pane from each of them
    /// would mean a database write and a tree broadcast per viewer.
    ///
    /// A clean exit closes the pane, as it does in any terminal — you typed
    /// `exit`, so the window goes. A failure leaves it, with its output and a
    /// Restart button, because that is the moment you most want to read it.
    pub async fn watch_exits(self: Arc<Self>) {
        let mut rx = self.ptys.subscribe();
        loop {
            match rx.recv().await {
                Ok(crate::pty::PtyEvent::Exited { pane, code }) => {
                    self.mark_exited(pane).await;
                    if code == 0 {
                        self.close_pane_inner(pane).await;
                    }
                }
                Ok(_) => {}
                // Lagged only drops output frames; exits are rare enough that
                // missing one would be a surprise, but carrying on is right.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
    }

    /// Closes a pane, and whatever it was the last of.
    ///
    /// Split out of the `ClosePane` handler because a shell exiting on its own
    /// needs the same thing to happen — including the cascade, which lives in
    /// `close_pane` returning `None` for the last pane in a tab.
    pub(crate) async fn close_pane_inner(&self, pane: PaneId) {
        let closed = self.tree.lock().await.close_pane(pane);
        match closed {
            Some(_) => {
                if let Some(pty) = self.pty_of(pane).await {
                    self.ptys.kill(pty);
                }
            }
            // Last pane in a tab: closing it means closing the tab.
            None => {
                let tab = self.tree.lock().await.tab_of(pane);
                if let Some(tab) = tab {
                    self.close_tab_inner(tab).await;
                }
            }
        }
        self.commit().await;
    }

    async fn close_tab_inner(&self, tab: TabId) {
        let ws = self.workspace_of_tab(tab).await;
        let panes = self.tree.lock().await.close_tab(tab);
        for pane in panes {
            if let Some(pty) = self.pty_of(pane).await {
                self.ptys.kill(pty);
            }
        }
        // A workspace whose last tab closed would linger as a ghost: invisible
        // in the UI (views hide tabless workspaces) yet counted by bootstrap,
        // so the next launch opened onto apparent emptiness with dead
        // shortcuts. Closing the workspace with its last tab is what the user
        // meant anyway.
        if let Some(ws) = ws {
            let mut tree = self.tree.lock().await;
            if tree
                .workspaces
                .iter()
                .find(|w| w.id == ws)
                .is_some_and(|w| w.tabs.is_empty())
            {
                tree.close_workspace(ws);
            }
        }
    }

    /// Cleans up the workspace tree at startup. Does **not** open a workspace:
    /// a fresh install comes up empty on purpose, and the UI prompts the owner
    /// to choose a folder rather than guessing one for them. Opening onto the
    /// launch directory picked whatever the daemon happened to be started in —
    /// often $HOME or `/`, almost never what the user wanted.
    pub async fn bootstrap(&self) -> Result<()> {
        // Self-heal databases written before the ghost-workspace fix: a
        // workspace with no tabs is invisible in every view but used to count
        // as "not empty", leaving the app stuck showing an empty stage while
        // the sidebar claimed a workspace existed.
        let mut tree = self.tree.lock().await;
        let ghosts: Vec<WsId> = tree
            .workspaces
            .iter()
            .filter(|w| w.tabs.is_empty())
            .map(|w| w.id)
            .collect();
        for ws in ghosts {
            tree.close_workspace(ws);
        }
        Ok(())
    }

    /// Restarts nothing on load, but every restored pane needs a process
    /// before it can be typed into.
    pub async fn resume_all(&self) -> Result<()> {
        let panes: Vec<PaneId> = {
            let tree = self.tree.lock().await;
            tree.workspaces
                .iter()
                .flat_map(|w| &w.tabs)
                .flat_map(|t| &t.panes)
                .map(|p| p.id)
                .collect()
        };
        for pane in panes {
            if let Err(e) = self.ensure_running(pane).await {
                tracing::warn!("pane {pane} failed to start: {e}");
            }
        }
        Ok(())
    }

    pub fn owner_key(&self) -> &str {
        &self.owner_key
    }

    /// Constant-time comparison: a length-or-prefix leak would let the key be
    /// guessed a character at a time.
    pub fn is_owner_key(&self, given: &str) -> bool {
        let want = self.owner_key.as_bytes();
        let got = given.as_bytes();
        if want.len() != got.len() {
            return false;
        }
        want.iter().zip(got).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
    }

    pub async fn owner_grant(&self) -> Grant {
        Grant {
            token: OWNER_TOKEN.into(),
            scope: Scope::All,
            writable: true,
            host: true,
        }
    }

    pub fn session_scope_label(scope: &Scope) -> String {
        match scope {
            Scope::All => "All workspaces".into(),
            Scope::Workspace(_) => "Workspace".into(),
            Scope::Tab(_) => "Tab".into(),
            Scope::Pane(_) => "Pane".into(),
        }
    }

    fn mint_secret() -> String {
        use rand::Rng;
        let mut rng = rand::rng();
        // 128 bits of hex.
        (0..4)
            .map(|_| format!("{:08x}", rng.random::<u32>()))
            .collect::<String>()
    }

    /// Records where the HTTP listener actually bound, so spawned PTYs can be
    /// pointed at the hook endpoint. Called once, before any UI-driven spawn.
    pub fn set_hook_port(&self, port: u16) {
        self.hook_port
            .store(port, std::sync::atomic::Ordering::Relaxed);
    }

    /// The bound port, for building share URLs.
    pub fn hook_port_now(&self) -> u16 {
        self.hook_port.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Per-pane hook secret, minted on first use and rotated on every spawn.
    /// Constant-time compared so a caller cannot probe it byte by byte.
    pub async fn hook_secret(&self, pane: PaneId) -> String {
        let mut secrets = self.hook_secrets.lock().await;
        secrets.entry(pane).or_insert_with(Self::mint_secret).clone()
    }

    pub async fn check_hook_secret(&self, pane: PaneId, given: &str) -> bool {
        let Some(want) = self.hook_secrets.lock().await.get(&pane).cloned() else {
            return false;
        };
        if want.len() != given.len() {
            return false;
        }
        want.bytes()
            .zip(given.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
    }

    pub fn is_exposed(&self) -> bool {
        self.exposed.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Opens or closes the port to non-loopback clients. In-memory only:
    /// after a restart sharing is off again, which is the safe default.
    ///
    /// Closing also disconnects every remote client already attached — the
    /// gate only filters *new* requests, and an open WebSocket would otherwise
    /// live on as if nothing happened. Disconnect, not revoke: these clients
    /// did nothing wrong, and must be able to return when sharing reopens.
    pub async fn set_exposed(&self, on: bool) {
        self.exposed.store(on, std::sync::atomic::Ordering::Relaxed);
        if !on {
            // `None`: no reason given, so the client keeps trying.
            self.close_conns(|c| !c.addr.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()), None)
                .await;
        }
        let _ = self.changed.send(TreeChanged);
    }

    pub fn subscribe_agent(&self) -> broadcast::Receiver<AgentDelta> {
        self.agent_tx.subscribe()
    }

    pub async fn agent_settings(&self) -> AgentSettings {
        *self.agent_settings.lock().await
    }

    /// Codex hooks readiness, probed lazily and cached. `codex features list`
    /// is the ground truth on a modern CLI; a codex without that subcommand
    /// is the legacy feature-flag era; no codex at all is "missing".
    pub fn codex_hooks_state(&self) -> &str {
        self.codex_hooks.get_or_init(|| {
            let out = std::process::Command::new("codex")
                .args(["features", "list"])
                .output();
            match out {
                Ok(o) if o.status.success() => {
                    let text = String::from_utf8_lossy(&o.stdout);
                    let stable = text.lines().any(|l| {
                        let mut it = l.split_whitespace();
                        it.next() == Some("hooks")
                            && l.contains("stable")
                            && l.trim_end().ends_with("true")
                    });
                    if stable { "stable".into() } else { "legacy".into() }
                }
                Ok(_) => "legacy".into(),
                Err(_) => "missing".into(),
            }
        })
    }

    /// Flips one toggle. DB first (single transaction, which also clears the
    /// agent's session ids when a resume toggle goes off), then memory, then
    /// the broadcast. Status-off additionally wipes that agent's live dots so
    /// clients hide them immediately — not on the next event (mux0's known
    /// stale-dot bug).
    pub async fn set_agent_setting(&self, agent: AgentKind, setting: AgentSetting, on: bool) {
        self.store
            .lock()
            .await
            .put_agent_setting(setting, agent, on)
            .expect("write state.db");
        let snapshot = {
            let mut s = self.agent_settings.lock().await;
            s.set(agent, setting, on);
            *s
        };

        if !on {
            match setting {
                AgentSetting::Status => self.clear_agent_status(Some(agent)).await,
                AgentSetting::Resume => self.clear_session_refs(Some(agent)).await,
            }
        }
        let _ = self.agent_tx.send(AgentDelta::Settings { settings: snapshot });
    }

    /// The Reset button: six toggles off, all resume ids gone, all dots gone.
    pub async fn reset_agent_settings(&self) {
        self.store.lock().await.reset_agent_settings().expect("write state.db");
        *self.agent_settings.lock().await = AgentSettings::default();
        self.clear_agent_status(None).await;
        self.clear_session_refs(None).await;
        let _ = self
            .agent_tx
            .send(AgentDelta::Settings { settings: AgentSettings::default() });
    }

    /// Resets live agent status for one agent (or all), broadcasting the
    /// now-empty views.
    async fn clear_agent_status(&self, agent: Option<AgentKind>) {
        let mut deltas = Vec::new();
        {
            let mut tree = self.tree.lock().await;
            for w in &mut tree.workspaces {
                for t in &mut w.tabs {
                    for p in &mut t.panes {
                        let hit = agent.is_none() || p.agent == agent;
                        if hit && p.status.view().phase != crate::proto::AgentPhase::NeverRan {
                            p.status = crate::agent::AgentState::default();
                            deltas.push(AgentDelta::Status {
                                pane: p.id,
                                status: p.status.view().clone(),
                            });
                        }
                    }
                }
            }
        }
        for d in deltas {
            let _ = self.agent_tx.send(d);
        }
    }

    /// Drops in-memory session ids (the DB rows were cleared in the settings
    /// transaction) so the next save cannot resurrect them.
    async fn clear_session_refs(&self, agent: Option<AgentKind>) {
        let mut tree = self.tree.lock().await;
        for w in &mut tree.workspaces {
            for t in &mut w.tabs {
                for p in &mut t.panes {
                    if agent.is_none() || p.agent == agent {
                        p.session_ref = None;
                    }
                }
            }
        }
    }

    /// Applies one normalized hook event to a pane. This is the only write
    /// path into agent state; the state machine decides what the event means
    /// and whether it is stale.
    ///
    /// Persistence is deliberately selective: session id and title matter
    /// across restarts, the phase does not — a restarted daemon has no idea
    /// what the agent is doing until its next hook.
    pub async fn apply_agent_event(&self, pane: PaneId, ev: AgentEvent) {
        let settings = *self.agent_settings.lock().await;
        let status_on = settings.status(ev.agent);
        let resume_on = settings.resume(ev.agent);

        let mut dirty = false;
        let mut deltas: Vec<AgentDelta> = Vec::new();
        {
            let mut tree = self.tree.lock().await;
            let Some(p) = tree.pane_mut(pane) else { return };

            // Status is gated per agent. With the toggle off the event is
            // not even applied, so re-enabling starts clean from the next
            // live event — exactly what the spec asks.
            if status_on && p.status.apply(&ev) {
                deltas.push(AgentDelta::Status { pane, status: p.status.view().clone() });
            }
            if p.agent != Some(ev.agent) {
                p.agent = Some(ev.agent);
                dirty = true;
            }
            // Resume gate: session ids are only stored while the toggle is
            // on. The read side re-checks at launch, so old rows cannot
            // sneak past either way.
            //
            // Newest wins: hooks race each other to the daemon, and a late one
            // from the session just left must not put its id back.
            if resume_on && ev.at_ms >= p.session_ref_at {
                if let Some(id) = &ev.session_id {
                    p.session_ref_at = ev.at_ms;
                    if p.session_ref.as_deref() != Some(id.as_str()) {
                        p.session_ref = Some(id.clone());
                        dirty = true;
                    }
                }
            }
            // An empty or missing title never erases a known one, and prompt
            // fallbacks never replace an existing (better) title. Transcript
            // titles do replace prompt fallbacks — TurnEnd wins.
            if let Some(t) = ev
                .session_title
                .as_ref()
                .filter(|t| !t.is_empty())
            {
                let replace = match &ev.kind {
                    crate::proto::AgentEventKind::TurnEnd { .. } => {
                        p.session_title.as_deref() != Some(t.as_str())
                    }
                    _ => p.session_title.is_none(),
                };
                if replace {
                    p.session_title = Some(t.clone());
                    dirty = true;
                    deltas.push(AgentDelta::SessionTitle { pane, text: t.clone() });
                }
            }
        }

        // Persist outside the tree lock — commit() takes it again.
        if dirty {
            self.commit().await;
        }
        for d in deltas {
            let _ = self.agent_tx.send(d);
        }
    }

    /// Called by the cwd poller. Broadcasts only on change — the tree resend
    /// is heavyweight and the poller ticks constantly.
    pub async fn update_cwd(&self, pane: PaneId, cwd: String, git: Option<crate::proto::GitInfo>) {
        let mut tree = self.tree.lock().await;
        let Some(p) = tree.pane_mut(pane) else { return };
        if p.cwd == cwd && p.git == git {
            return;
        }
        // Directory moves are persisted (git churn alone is not — it changes
        // every commit and is rederived at startup anyway). Without this a
        // restart respawns the pane in the directory it was *created* in, and
        // an auto-resumed agent lands in the wrong project — Claude then
        // greets the user with a trust prompt for a folder they left long ago.
        let dir_changed = p.cwd != cwd;
        p.cwd = cwd.clone();
        p.git = git.clone();
        drop(tree);
        if dir_changed {
            let tree = self.tree.lock().await;
            self.store.lock().await.save_tree(&tree).expect("write state.db");
        }
        let _ = self.agent_tx.send(AgentDelta::Cwd { pane, path: cwd, git });
    }

    /// Registers a live socket and returns its session id, with the receiver
    /// that fires when it is to be closed.
    pub async fn add_conn(
        &self,
        grant: &Grant,
        addr: String,
    ) -> (SessionId, tokio::sync::oneshot::Receiver<Option<CloseReason>>) {
        let session = {
            let mut n = self.next_session.lock().await;
            *n += 1;
            *n
        };
        let (close, closed) = tokio::sync::oneshot::channel();
        let conn = Conn { token: grant.token.clone(), addr, close: Some(close) };
        self.conns.lock().await.insert(session, conn);
        let _ = self.changed.send(TreeChanged);
        (session, closed)
    }

    pub async fn remove_conn(&self, session: SessionId) {
        self.conns.lock().await.remove(&session);
        let _ = self.changed.send(TreeChanged);
    }

    /// Deletes a link and closes whatever is attached through it. A link
    /// belongs to the one device that paired on it, so this is how a device
    /// is disconnected for good: its secret now matches nothing.
    pub async fn revoke(&self, token: &str) {
        self.store.lock().await.delete_grant(token).expect("write state.db");
        self.close_conns(|c| c.token == token, Some(CloseReason::Revoked)).await;
    }

    /// Every link, and everyone on them. The owner's own windows stay.
    pub async fn revoke_all(&self) {
        self.store.lock().await.delete_all_grants().expect("write state.db");
        self.close_conns(|c| c.token != OWNER_TOKEN, Some(CloseReason::Revoked)).await;
    }

    /// Closes the matching sockets. With a reason the client is told and
    /// stops; without one it reconnects as after any dropped connection.
    async fn close_conns(&self, which: impl Fn(&Conn) -> bool, why: Option<CloseReason>) {
        let mut conns = self.conns.lock().await;
        for conn in conns.values_mut().filter(|c| which(c)) {
            if let Some(close) = conn.close.take() {
                let _ = close.send(why);
            }
        }
        conns.retain(|_, c| c.close.is_some());
        drop(conns);
        let _ = self.changed.send(TreeChanged);
    }

    /// Every device holding a link, for the connection manager — online or
    /// not. A phone in the background still has access, and it is access the
    /// owner needs to see, not who happens to be looking right now.
    pub async fn peers(&self) -> Vec<Peer> {
        let devices = self.store.lock().await.paired_devices().expect("read state.db");
        let conns = self.conns.lock().await;
        devices
            .into_iter()
            .map(|d| Peer {
                addr: conns.values().find(|c| c.token == d.token).map(|c| c.addr.clone()),
                scope: Self::session_scope_label(&d.scope),
                writable: d.writable,
                device: d.device,
                paired_at: d.paired_at,
                token: d.token,
            })
            .collect()
    }

    pub async fn workspace_of_tab(&self, tab: TabId) -> Option<WsId> {
        let tree = self.tree.lock().await;
        tree.workspaces
            .iter()
            .find(|w| w.tabs.iter().any(|t| t.id == tab))
            .map(|w| w.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{AgentEventKind, Dir, Shelf};
    use crate::share::Scope;

    /// A ready app with one workspace open, as most tests assume. `bootstrap`
    /// no longer opens one itself (a fresh install waits for the user to choose
    /// a folder), so tests open theirs explicitly.
    async fn app() -> Arc<App> {
        let a = App::new(Store::in_memory().unwrap(), 1000);
        a.bootstrap().await.unwrap();
        a.handle_host(&a.owner_grant().await, In::OpenWorkspace { path: "/tmp".into() })
            .await
            .unwrap();
        a
    }

    fn shared(scope: Scope, writable: bool) -> Grant {
        Grant { token: "t".into(), scope, writable, host: false }
    }

    #[tokio::test]
    async fn opening_a_workspace_gives_a_usable_pane() {
        let a = app().await;
        let tree = a.tree.lock().await;
        assert_eq!(tree.workspaces.len(), 1);
        assert_eq!(tree.workspaces[0].tabs.len(), 1);
        assert_eq!(tree.workspaces[0].tabs[0].panes.len(), 1);
        // A pane is only useful once it has a process.
        assert!(tree.workspaces[0].tabs[0].panes[0].pty.is_some());
    }

    #[tokio::test]
    async fn bootstrap_opens_nothing_on_a_fresh_install() {
        // The behaviour change: a clean database comes up empty, and the UI
        // prompts the owner to choose a folder instead of guessing one.
        let a = App::new(Store::in_memory().unwrap(), 1000);
        a.bootstrap().await.unwrap();
        assert!(a.tree.lock().await.workspaces.is_empty());
    }

    #[tokio::test]
    async fn owner_can_split_and_the_new_pane_runs() {
        let a = app().await;
        let owner = a.owner_grant().await;
        let pane = {
            let t = a.tree.lock().await;
            t.workspaces[0].tabs[0].panes[0].id
        };

        a.handle_host(&owner, In::Split { pane, dir: Dir::Vertical })
            .await
            .unwrap();

        let t = a.tree.lock().await;
        assert_eq!(t.workspaces[0].tabs[0].panes.len(), 2);
        assert!(t.workspaces[0].tabs[0].panes.iter().all(|p| p.pty.is_some()));
    }

    /// The hole the architecture review found: a writable pane-scoped share
    /// must not be able to restructure anything.
    #[tokio::test]
    async fn a_shared_pane_cannot_spawn_or_destroy() {
        let a = app().await;
        let (ws, tab, pane) = {
            let t = a.tree.lock().await;
            (
                t.workspaces[0].id,
                t.workspaces[0].tabs[0].id,
                t.workspaces[0].tabs[0].panes[0].id,
            )
        };
        let viewer = shared(Scope::Pane(pane), true);

        for msg in [
            In::Split { pane, dir: Dir::Vertical },
            In::OpenTab { ws },
            In::CloseTab { tab },
            In::ClosePane { pane },
            In::CloseWorkspace { ws },
            In::OpenWorkspace { path: "/etc".into() },
        ] {
            a.handle_host(&viewer, msg).await.unwrap();
        }

        let t = a.tree.lock().await;
        assert_eq!(t.workspaces.len(), 1, "workspace count changed");
        assert_eq!(t.workspaces[0].tabs.len(), 1, "tab count changed");
        assert_eq!(t.workspaces[0].tabs[0].panes.len(), 1, "pane count changed");
    }

    #[tokio::test]
    async fn only_the_owner_resizes_the_pty() {
        let a = app().await;
        let pane = {
            let t = a.tree.lock().await;
            t.workspaces[0].tabs[0].panes[0].id
        };

        // A read-only viewer reaching in would repaint the owner's TUI.
        let viewer = shared(Scope::Pane(pane), false);
        assert!(a.set_viewport(&viewer, false, pane, 40, 10).await.is_none());

        // So would a writable one: size is not a write, it is ownership.
        let writable_viewer = shared(Scope::Pane(pane), true);
        assert!(a.set_viewport(&writable_viewer, false, pane, 40, 10).await.is_none());
        // ...unless the link it came from was granted sizing: a phone cannot
        // read a terminal laid out for a screen it does not have.
        assert_eq!(
            a.set_viewport(&writable_viewer, true, pane, 40, 10).await,
            Some((40, 10)),
            "a link with sizing rights may resize the terminal"
        );

        let owner = a.owner_grant().await;
        assert_eq!(a.set_viewport(&owner, false, pane, 96, 38).await, Some((96, 38)));
        assert_eq!(a.tree.lock().await.pane(pane).unwrap().cols, 96);
    }

    #[tokio::test]
    async fn caps_trim_chrome_to_the_scope() {
        let a = app().await;
        let pane = {
            let t = a.tree.lock().await;
            t.workspaces[0].tabs[0].panes[0].id
        };

        let (_, owner_caps) = a.view_for(&a.owner_grant().await).await;
        assert!(owner_caps.show_sidebar && owner_caps.show_tabs && owner_caps.host);

        let (view, caps) = a.view_for(&shared(Scope::Pane(pane), false)).await;
        assert!(!caps.show_sidebar, "a pane share must not reveal the sidebar");
        assert!(!caps.show_tabs);
        assert!(!caps.host && !caps.may_open_tab && !caps.writable);
        assert_eq!(view.workspaces[0].tabs[0].panes.len(), 1);
    }

    /// The connection-time replay in http.rs skips a barely-started pane for
    /// the owner only — whose browser issued the spawn and would double-print
    /// the prompt — gated on `may_administer()`. A share must therefore not
    /// count as the owner, however writable, or a viewer switching to a tab
    /// the owner never opened gets a blank sheet: its resumed-but-quiet pane
    /// sits under the replay threshold.
    #[tokio::test]
    async fn a_workspace_viewer_is_not_the_owner_and_sees_every_tab() {
        let a = app().await;
        let ws = a.tree.lock().await.workspaces[0].id;

        // A second tab the owner has "opened" but that has produced little —
        // exactly the case that used to replay as blank for a viewer.
        a.handle_host(&a.owner_grant().await, In::OpenTab { ws })
            .await
            .unwrap();
        assert_eq!(a.tree.lock().await.workspaces[0].tabs.len(), 2);

        for writable in [false, true] {
            let viewer = shared(Scope::Workspace(ws), writable);
            // The replay gate keys on this: a viewer never spawned anything,
            // so it must not inherit the owner's just-spawned skip.
            assert!(
                !viewer.host,
                "a workspace share (writable={writable}) must not count as owner",
            );
            // Both tabs' panes are in scope, so both are replayable — the
            // switch has something to show.
            assert_eq!(a.visible(&viewer).await.len(), 2);
        }
    }

    #[tokio::test]
    async fn a_tab_viewer_sees_a_newly_split_pane() {
        // The delivery half of the review's C1: authorisation grows with the
        // tab, and the tree resend is what tells the viewer about it.
        let a = app().await;
        let (tab, pane) = {
            let t = a.tree.lock().await;
            (t.workspaces[0].tabs[0].id, t.workspaces[0].tabs[0].panes[0].id)
        };
        let viewer = shared(Scope::Tab(tab), false);
        assert_eq!(a.visible(&viewer).await.len(), 1);

        let mut rx = a.subscribe_tree();
        a.handle_host(&a.owner_grant().await, In::Split { pane, dir: Dir::Horizontal })
            .await
            .unwrap();

        assert!(rx.try_recv().is_ok(), "structural change must be announced");
        assert_eq!(a.visible(&viewer).await.len(), 2);
        let (view, _) = a.view_for(&viewer).await;
        assert_eq!(view.workspaces[0].tabs[0].panes.len(), 2);
    }

    #[tokio::test]
    async fn activating_a_workspace_lands_on_one_of_its_tabs() {
        // Clicking a workspace sends Activate{tab: None}. Keeping the old
        // workspace's tab id made no tab in the new workspace "active", so
        // the tab bar lost its highlight entirely.
        let a = app().await;
        let owner = a.owner_grant().await;
        let ws1 = a.tree.lock().await.workspaces[0].id;
        a.handle_host(&owner, In::OpenWorkspace { path: "/tmp".into() }).await.unwrap();
        let (ws2, tab2) = {
            let t = a.tree.lock().await;
            (t.workspaces[1].id, t.workspaces[1].tabs[0].id)
        };
        assert_eq!(a.tree.lock().await.active_tab, Some(tab2));

        // Back to workspace 1 by clicking it (no tab named).
        a.handle_host(&owner, In::Activate { ws: ws1, tab: None }).await.unwrap();
        let t = a.tree.lock().await;
        let active_tab = t.active_tab.expect("a tab must be active");
        assert!(
            t.workspaces[0].tabs.iter().any(|x| x.id == active_tab),
            "the active tab must belong to the activated workspace"
        );
        drop(t);

        // And going forward again keeps ws2's tab.
        a.handle_host(&owner, In::Activate { ws: ws2, tab: None }).await.unwrap();
        assert_eq!(a.tree.lock().await.active_tab, Some(tab2));
    }

    #[tokio::test]
    async fn a_cwd_move_survives_a_restart() {
        // The resume gap: the session id was remembered but the directory was
        // not, so `claude --resume` ran in the pane's *original* folder and
        // Claude opened with a trust prompt for the wrong project.
        let a = app().await;
        let pane = {
            let t = a.tree.lock().await;
            t.workspaces[0].tabs[0].panes[0].id
        };
        a.update_cwd(pane, "/private/tmp/project".into(), None).await;

        let reloaded = a.store.lock().await.load_tree().unwrap();
        assert_eq!(
            reloaded.workspaces[0].tabs[0].panes[0].cwd,
            "/private/tmp/project",
            "the moved-to directory must be what a restart spawns into"
        );
    }

    #[tokio::test]
    async fn each_workspace_remembers_its_own_last_tab() {
        // Switching projects and coming back used to dump you on the first tab,
        // because "which tab is in front" was one value shared by every
        // workspace: the id left over from the other project matched nothing
        // here, so the fallback picked tab one.
        let a = app().await;
        let owner = a.owner_grant().await;
        let ws1 = a.tree.lock().await.workspaces[0].id;

        a.handle_host(&owner, In::OpenTab { ws: ws1 }).await.unwrap();
        let second = {
            let t = a.tree.lock().await;
            t.workspaces[0].tabs[1].id
        };
        a.handle_host(&owner, In::Activate { ws: ws1, tab: Some(second) }).await.unwrap();

        // Away to another project...
        a.handle_host(&owner, In::OpenWorkspace { path: "/tmp/other".into() }).await.unwrap();
        let ws2 = a.tree.lock().await.workspaces[1].id;
        assert_ne!(ws1, ws2);

        // ...and back, by clicking the workspace (no tab named).
        a.handle_host(&owner, In::Activate { ws: ws1, tab: None }).await.unwrap();
        assert_eq!(
            a.tree.lock().await.active_tab,
            Some(second),
            "returning to a workspace lands on the tab it was last showing"
        );
    }

    #[tokio::test]
    async fn shelving_a_tab_keeps_its_terminal_running() {
        // The whole point of setting a tab aside rather than closing it: the
        // agent in there carries on, and is still there when you come back.
        let a = app().await;
        let owner = a.owner_grant().await;
        let (tab, pane) = {
            let t = a.tree.lock().await;
            (t.workspaces[0].tabs[0].id, t.workspaces[0].tabs[0].panes[0].id)
        };
        a.ensure_running(pane).await.unwrap();
        let live = a.ptys.live_count();
        assert_eq!(live, 1);

        a.handle_host(&owner, In::ShelveTab { tab, shelf: Some(Shelf::Archive) }).await.unwrap();

        assert_eq!(a.ptys.live_count(), live, "shelving must not kill a pty");
        let t = a.tree.lock().await;
        assert_eq!(t.workspaces[0].tabs[0].shelf, Some(Shelf::Archive));
        assert_eq!(t.workspaces[0].tabs.len(), 1, "the tab is set aside, not removed");
    }

    #[tokio::test]
    async fn shelving_the_current_tab_moves_you_off_it() {
        // You cannot be left looking at a tab that is no longer on the strip.
        let a = app().await;
        let owner = a.owner_grant().await;
        let ws = a.tree.lock().await.workspaces[0].id;
        a.handle_host(&owner, In::OpenTab { ws }).await.unwrap();
        let (first, second) = {
            let t = a.tree.lock().await;
            (t.workspaces[0].tabs[0].id, t.workspaces[0].tabs[1].id)
        };
        assert_eq!(a.tree.lock().await.active_tab, Some(second));

        a.handle_host(&owner, In::ShelveTab { tab: second, shelf: Some(Shelf::Archive) }).await.unwrap();
        assert_eq!(a.tree.lock().await.active_tab, Some(first));

        // And taking it back brings you to it.
        a.handle_host(&owner, In::ShelveTab { tab: second, shelf: None }).await.unwrap();
        assert_eq!(a.tree.lock().await.active_tab, Some(second));
    }

    #[tokio::test]
    async fn a_whole_machine_link_may_open_a_tab_and_nothing_more() {
        // The widest share there is — every workspace, writable. It may open
        // a tab, because someone working across the machine needs another
        // terminal sometimes and the result lands where they can see it. It
        // may not file the owner's tabs, close their workspaces, or open the
        // port: handing out a link is not handing over the machine.
        let a = app().await;
        let owner = a.owner_grant().await;
        let (ws, tab) = {
            let t = a.tree.lock().await;
            (t.workspaces[0].id, t.workspaces[0].tabs[0].id)
        };
        let guest = Grant {
            token: "guest".into(),
            scope: Scope::All,
            writable: true,
            host: false,
        };

        let tabs = a.tree.lock().await.workspaces[0].tabs.len();
        a.handle_host(&guest, In::OpenTab { ws }).await.unwrap();
        assert_eq!(
            a.tree.lock().await.workspaces[0].tabs.len(),
            tabs + 1,
            "the one thing a link may do",
        );

        a.handle_host(&guest, In::ShelveTab { tab, shelf: Some(Shelf::Archive) })
            .await
            .unwrap();
        assert_eq!(
            a.tree.lock().await.workspaces[0].tabs[0].shelf,
            None,
            "filing is the owner's own organisation of their work",
        );

        let before = a.tree.lock().await.workspaces.len();
        a.handle_host(&guest, In::CloseWorkspace { ws }).await.unwrap();
        assert_eq!(a.tree.lock().await.workspaces.len(), before);

        a.handle_host(&guest, In::SetWebServer { exposed: true }).await.unwrap();
        assert!(!a.is_exposed(), "a link must not open the port");

        // The owner, through the same door, is obeyed.
        a.handle_host(&owner, In::ShelveTab { tab, shelf: Some(Shelf::Archive) })
            .await
            .unwrap();
        assert_eq!(
            a.tree.lock().await.workspaces[0].tabs[0].shelf,
            Some(Shelf::Archive),
        );
    }

    #[tokio::test]
    async fn a_narrower_link_cannot_even_open_a_tab() {
        // The exception is whole-machine scope only. Anywhere narrower, the
        // tab would land somewhere the person who asked cannot see.
        let a = app().await;
        let ws = a.tree.lock().await.workspaces[0].id;
        let guest = Grant {
            token: "guest".into(),
            scope: Scope::Workspace(ws),
            writable: true,
            host: false,
        };

        let tabs = a.tree.lock().await.workspaces[0].tabs.len();
        a.handle_host(&guest, In::OpenTab { ws }).await.unwrap();
        assert_eq!(a.tree.lock().await.workspaces[0].tabs.len(), tabs);
    }

    #[tokio::test]
    async fn the_two_shelves_are_told_apart() {
        // Filing only: a tab moves between shelves and back to the strip
        // without anything else about it changing.
        let a = app().await;
        let owner = a.owner_grant().await;
        let tab = a.tree.lock().await.workspaces[0].tabs[0].id;

        for shelf in [Some(Shelf::Archive), Some(Shelf::Later), None] {
            a.handle_host(&owner, In::ShelveTab { tab, shelf }).await.unwrap();
            assert_eq!(a.tree.lock().await.workspaces[0].tabs[0].shelf, shelf);
        }
    }

    #[tokio::test]
    async fn shelving_every_tab_keeps_the_workspace() {
        // Closing the last tab takes its workspace along; setting them all
        // aside must not, or the den would be a way to lose a project.
        let a = app().await;
        let owner = a.owner_grant().await;
        let (ws, tab) = {
            let t = a.tree.lock().await;
            (t.workspaces[0].id, t.workspaces[0].tabs[0].id)
        };

        a.handle_host(&owner, In::ShelveTab { tab, shelf: Some(Shelf::Archive) }).await.unwrap();

        let t = a.tree.lock().await;
        assert_eq!(t.workspaces.len(), 1, "the workspace survives an empty strip");
        assert_eq!(t.workspaces[0].id, ws);
        assert_eq!(t.active_tab, None, "nothing on the strip to be active");
    }

    #[tokio::test]
    async fn a_shelved_tab_is_never_activated_by_fallback() {
        // Clicking the workspace must land on a tab you can actually see.
        let a = app().await;
        let owner = a.owner_grant().await;
        let ws = a.tree.lock().await.workspaces[0].id;
        a.handle_host(&owner, In::OpenTab { ws }).await.unwrap();
        let (first, second) = {
            let t = a.tree.lock().await;
            (t.workspaces[0].tabs[0].id, t.workspaces[0].tabs[1].id)
        };

        // Set the *first* aside, then ask for the workspace with no tab named.
        a.handle_host(&owner, In::ShelveTab { tab: first, shelf: Some(Shelf::Archive) }).await.unwrap();
        a.handle_host(&owner, In::Activate { ws, tab: None }).await.unwrap();

        assert_eq!(a.tree.lock().await.active_tab, Some(second));
    }

    #[tokio::test]
    async fn closing_a_tab_returns_to_the_one_you_came_from() {
        // Eight tabs, sitting on the fourth, step to the fifth, close it. You
        // came from the fourth, so that is where you land. Falling back to the
        // first tab — which is what this did — threw you across the whole strip
        // to somewhere you had never been.
        let a = app().await;
        let owner = a.owner_grant().await;
        let ws = a.tree.lock().await.workspaces[0].id;

        for _ in 0..7 {
            a.handle_host(&owner, In::OpenTab { ws }).await.unwrap();
        }
        let ids: Vec<TabId> = a.tree.lock().await.workspaces[0]
            .tabs
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids.len(), 8);

        a.handle_host(&owner, In::Activate { ws, tab: Some(ids[3]) }).await.unwrap();
        a.handle_host(&owner, In::Activate { ws, tab: Some(ids[4]) }).await.unwrap();
        a.handle_host(&owner, In::CloseTab { tab: ids[4] }).await.unwrap();

        assert_eq!(
            a.tree.lock().await.active_tab,
            Some(ids[3]),
            "closing a tab lands on the one you were looking at before it"
        );
    }

    #[tokio::test]
    async fn closing_walks_back_through_tabs_you_actually_visited() {
        // The tab you came from may itself be gone. Each close steps one
        // further back through where you have been, never to a tab you never
        // opened.
        let a = app().await;
        let owner = a.owner_grant().await;
        let ws = a.tree.lock().await.workspaces[0].id;

        for _ in 0..3 {
            a.handle_host(&owner, In::OpenTab { ws }).await.unwrap();
        }
        let ids: Vec<TabId> = a.tree.lock().await.workspaces[0]
            .tabs
            .iter()
            .map(|t| t.id)
            .collect();

        // Visit second, then third, then fourth.
        for i in [1usize, 2, 3] {
            a.handle_host(&owner, In::Activate { ws, tab: Some(ids[i]) }).await.unwrap();
        }

        a.handle_host(&owner, In::CloseTab { tab: ids[3] }).await.unwrap();
        assert_eq!(a.tree.lock().await.active_tab, Some(ids[2]));

        a.handle_host(&owner, In::CloseTab { tab: ids[2] }).await.unwrap();
        assert_eq!(a.tree.lock().await.active_tab, Some(ids[1]));
    }

    #[tokio::test]
    async fn closing_a_tab_you_are_not_on_leaves_you_where_you_are() {
        // Only the tab in front decides where you go. Closing some other tab —
        // from its own × — must not move you at all.
        let a = app().await;
        let owner = a.owner_grant().await;
        let ws = a.tree.lock().await.workspaces[0].id;

        for _ in 0..2 {
            a.handle_host(&owner, In::OpenTab { ws }).await.unwrap();
        }
        let ids: Vec<TabId> = a.tree.lock().await.workspaces[0]
            .tabs
            .iter()
            .map(|t| t.id)
            .collect();

        a.handle_host(&owner, In::Activate { ws, tab: Some(ids[2]) }).await.unwrap();
        a.handle_host(&owner, In::CloseTab { tab: ids[0] }).await.unwrap();

        assert_eq!(a.tree.lock().await.active_tab, Some(ids[2]));
    }

    #[tokio::test]
    async fn a_remembered_tab_that_was_closed_falls_back() {
        let a = app().await;
        let owner = a.owner_grant().await;
        let ws = a.tree.lock().await.workspaces[0].id;

        a.handle_host(&owner, In::OpenTab { ws }).await.unwrap();
        let (first, second) = {
            let t = a.tree.lock().await;
            (t.workspaces[0].tabs[0].id, t.workspaces[0].tabs[1].id)
        };
        a.handle_host(&owner, In::Activate { ws, tab: Some(second) }).await.unwrap();
        a.handle_host(&owner, In::CloseTab { tab: second }).await.unwrap();

        a.handle_host(&owner, In::Activate { ws, tab: None }).await.unwrap();
        assert_eq!(
            a.tree.lock().await.active_tab,
            Some(first),
            "a remembered tab that no longer exists must not leave the bar empty"
        );
    }

    #[tokio::test]
    async fn a_late_hook_cannot_put_back_the_session_just_left() {
        let a = app().await;
        a.set_agent_setting(AgentKind::Claude, AgentSetting::Resume, true).await;
        let pane = a.tree.lock().await.workspaces[0].tabs[0].panes[0].id;
        let ev = |id: &str, at_ms| AgentEvent {
            agent: AgentKind::Claude,
            kind: AgentEventKind::SessionEnd,
            at_ms,
            session_id: Some(id.into()),
            session_title: None,
        };

        a.apply_agent_event(pane, ev("new", 200)).await;
        // The old session's last word, arriving after the new one's first.
        a.apply_agent_event(pane, ev("old", 100)).await;

        let t = a.tree.lock().await;
        assert_eq!(t.pane(pane).unwrap().session_ref.as_deref(), Some("new"));
    }

    #[tokio::test]
    async fn a_new_tab_opens_on_the_workspace_folder() {
        // ⌘T sends only {open_tab, ws}: the folder is the workspace's, never
        // wherever the last shell happened to have cd'd to. Moving the existing
        // pane first is the whole point — with both at /tmp the assert would
        // pass even if the new tab started inheriting a cwd.
        //
        // Asserted at creation time. The poller that would otherwise overwrite
        // this (cwd::poll_forever) is spawned by main, not by App, so it does
        // not run here.
        let a = app().await;
        let owner = a.owner_grant().await;
        let (ws, old) = {
            let t = a.tree.lock().await;
            (t.workspaces[0].id, t.workspaces[0].tabs[0].panes[0].id)
        };
        a.update_cwd(old, "/private/tmp/elsewhere".into(), None).await;

        a.handle_host(&owner, In::OpenTab { ws }).await.unwrap();

        let t = a.tree.lock().await;
        let fresh = t.workspaces[0].tabs.last().unwrap();
        assert_eq!(
            fresh.panes[0].cwd, "/tmp",
            "a new tab opens on the workspace folder, not on the last shell's cwd"
        );
        // No session to resume, so nothing can redirect it to another folder.
        assert!(fresh.panes[0].session_ref.is_none());
    }

    #[tokio::test]
    async fn a_split_stays_where_you_were_looking() {
        // The opposite rule to the one above, pinned here so nobody "unifies"
        // the two: a split continues the pane it came from, cwd included.
        let a = app().await;
        let owner = a.owner_grant().await;
        let src = {
            let t = a.tree.lock().await;
            t.workspaces[0].tabs[0].panes[0].id
        };
        a.update_cwd(src, "/private/tmp/elsewhere".into(), None).await;

        a.handle_host(&owner, In::Split { pane: src, dir: Dir::Vertical })
            .await
            .unwrap();

        let t = a.tree.lock().await;
        let panes = &t.workspaces[0].tabs[0].panes;
        let fresh = panes.iter().find(|p| p.id != src).expect("split made a pane");
        assert_eq!(
            fresh.cwd, "/private/tmp/elsewhere",
            "a split inherits the source pane's cwd"
        );
    }

    #[tokio::test]
    async fn closing_the_last_tab_takes_its_workspace_along() {
        // The resume-killer: a tabless workspace survived in the database,
        // bootstrap saw "not empty", and the next launch had no panes at all.
        let a = app().await;
        let owner = a.owner_grant().await;
        let pane = {
            let t = a.tree.lock().await;
            t.workspaces[0].tabs[0].panes[0].id
        };
        a.handle_host(&owner, In::ClosePane { pane }).await.unwrap();

        assert!(a.tree.lock().await.workspaces.is_empty(), "ghost workspace left behind");
        let reloaded = a.store.lock().await.load_tree().unwrap();
        assert!(reloaded.workspaces.is_empty(), "ghost persisted to the database");
    }

    #[tokio::test]
    async fn bootstrap_heals_a_database_with_ghost_workspaces() {
        // Databases written before the fix hold workspaces with no tabs. They
        // must be cleared, and — since bootstrap no longer opens a replacement —
        // the app comes up empty, which the UI turns into a "choose a folder"
        // prompt rather than a dead "No workspace open" screen.
        let store = Store::in_memory().unwrap();
        let mut t = SessionTree::default();
        t.open_workspace("/a".into(), "a".into());
        t.open_workspace("/b".into(), "b".into());
        store.save_tree(&t).unwrap();

        let a = App::new(store, 100);
        a.bootstrap().await.unwrap();
        assert!(
            a.tree.lock().await.workspaces.is_empty(),
            "tabless ghosts cleared and nothing opened in their place",
        );
    }

    #[tokio::test]
    async fn bootstrap_keeps_a_real_workspace() {
        // A workspace with a tab is real work; bootstrap's ghost sweep must not
        // touch it.
        let a = App::new(Store::in_memory().unwrap(), 100);
        a.handle_host(&a.owner_grant().await, In::OpenWorkspace { path: "/keep".into() })
            .await
            .unwrap();
        a.bootstrap().await.unwrap();
        assert_eq!(a.tree.lock().await.workspaces.len(), 1, "real workspace survived bootstrap");
    }

    #[tokio::test]
    async fn closing_the_last_pane_closes_its_tab() {
        let a = app().await;
        let owner = a.owner_grant().await;
        let ws = a.tree.lock().await.workspaces[0].id;
        a.handle_host(&owner, In::OpenTab { ws }).await.unwrap();
        assert_eq!(a.tree.lock().await.workspaces[0].tabs.len(), 2);

        let pane = {
            let t = a.tree.lock().await;
            t.workspaces[0].tabs[1].panes[0].id
        };
        a.handle_host(&owner, In::ClosePane { pane }).await.unwrap();
        assert_eq!(a.tree.lock().await.workspaces[0].tabs.len(), 1);
    }

    #[tokio::test]
    async fn layout_is_persisted_on_every_change() {
        let a = app().await;
        let pane = {
            let t = a.tree.lock().await;
            t.workspaces[0].tabs[0].panes[0].id
        };
        a.handle_host(&a.owner_grant().await, In::Split { pane, dir: Dir::Vertical })
            .await
            .unwrap();

        let reloaded = a.store.lock().await.load_tree().unwrap();
        assert_eq!(reloaded.workspaces[0].tabs[0].panes.len(), 2);
    }

    #[tokio::test]
    async fn a_clean_exit_closes_its_pane() {
        // What `exit` does in any terminal: the window goes with the shell.
        // End to end through the watcher, so the exit status is the child's
        // own rather than something the test supplied.
        let a = app().await;
        let owner = a.owner_grant().await;
        let ws = a.tree.lock().await.workspaces[0].id;
        // A second tab, so closing this pane cannot take the workspace with it.
        a.handle_host(&owner, In::OpenTab { ws }).await.unwrap();
        let (doomed_tab, pane) = {
            let t = a.tree.lock().await;
            let tab = &t.workspaces[0].tabs[1];
            (tab.id, tab.panes[0].id)
        };

        let watcher = tokio::spawn(a.clone().watch_exits());
        let pty = a
            .ptys
            .spawn(crate::pty::Spawn {
                pane,
                cmd: vec!["sh".into(), "-c".into(), "exit 0".into()],
                cwd: "/tmp".into(),
                cols: 80,
                rows: 24,
                env: vec![],
                scrollback_lines: 100,
            })
            .unwrap();
        a.tree.lock().await.pane_mut(pane).unwrap().pty = Some(pty);

        for _ in 0..40 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if a.tree.lock().await.tab(doomed_tab).is_none() {
                break;
            }
        }
        watcher.abort();

        let t = a.tree.lock().await;
        assert!(t.pane(pane).is_none(), "a clean exit closes the pane");
        assert!(t.tab(doomed_tab).is_none(), "and the tab it was alone in");
        assert_eq!(t.workspaces.len(), 1, "the workspace still has its first tab");
    }

    #[tokio::test]
    async fn a_failed_exit_leaves_the_pane_to_be_read() {
        // The output is the reason you are looking, so a non-zero status keeps
        // the pane and its Restart button.
        let a = app().await;
        let pane = {
            let t = a.tree.lock().await;
            t.workspaces[0].tabs[0].panes[0].id
        };

        let watcher = tokio::spawn(a.clone().watch_exits());
        a.ptys
            .spawn(crate::pty::Spawn {
                pane,
                cmd: vec!["sh".into(), "-c".into(), "exit 3".into()],
                cwd: "/tmp".into(),
                cols: 80,
                rows: 24,
                env: vec![],
                scrollback_lines: 100,
            })
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(900)).await;
        watcher.abort();

        let t = a.tree.lock().await;
        assert!(t.pane(pane).is_some(), "a failure leaves the pane in place");
    }

    #[tokio::test]
    async fn exited_pane_survives_its_process() {
        let a = app().await;
        let pane = {
            let t = a.tree.lock().await;
            t.workspaces[0].tabs[0].panes[0].id
        };
        a.mark_exited(pane).await;

        let t = a.tree.lock().await;
        let p = t.pane(pane).unwrap();
        assert!(p.pty.is_none());
        // A PTY exit is not a turn result; the agent dot must be untouched.
        assert_eq!(p.status.view().phase, crate::proto::AgentPhase::NeverRan);
        drop(t);

        // And it can be re-run, which is the whole point of the split ids.
        a.handle_host(&a.owner_grant().await, In::Respawn { pane })
            .await
            .unwrap();
        assert!(a.tree.lock().await.pane(pane).unwrap().pty.is_some());
    }
}
