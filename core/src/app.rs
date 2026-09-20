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
    Peer, PtyId, SessionId, TabId, TreeView, WsId,
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

/// One live WebSocket. This is what the connection manager lists, and what a
/// kick has to be able to reach — revoking a grant row is not enough on its
/// own, because the socket it authorised is already open.
#[derive(Debug)]
pub struct Conn {
    pub session: SessionId,
    pub label: String,
    pub device: String,
    pub addr: String,
    pub scope: String,
    pub writable: bool,
    pub since: std::time::Instant,
    /// Fires when this connection is kicked. Dropping the sender is the
    /// signal, so a kick closes the socket immediately rather than waiting for
    /// the next broadcast to come round.
    kick: Option<tokio::sync::oneshot::Sender<()>>,
    /// Identity across reconnects, so a kick can be made to stick.
    key: String,
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
    /// disconnect — so the manager shows what is actually attached rather than
    /// what was ever granted.
    conns: Mutex<HashMap<SessionId, Conn>>,
    next_session: Mutex<SessionId>,
    /// Clients that have been kicked. Without this a kick lasts milliseconds:
    /// the client's reconnect loop simply comes straight back. Keyed by what
    /// identifies a client when there are no accounts — its grant, address and
    /// device.
    banned: Mutex<std::collections::HashSet<String>>,
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
        let tree = store.load_tree().unwrap_or_default();
        let agent_settings = store.load_agent_settings().unwrap_or_default();
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
            banned: Mutex::new(std::collections::HashSet::new()),
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
        if let Err(e) = self.store.lock().await.save_tree(&tree) {
            tracing::warn!("failed to persist layout: {e}");
        }
        drop(tree);
        let _ = self.changed.send(TreeChanged);
    }

    pub async fn view_for(&self, grant: &Grant) -> (TreeView, Caps) {
        let tree = self.tree.lock().await;
        let visible = visible_panes(&grant.scope, &tree);
        let view = tree.to_view(&visible);
        let caps = Caps {
            writable: grant.writable,
            owner: grant.may_mutate(),
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

        // Interactive, not login. A login shell re-runs the full profile in
        // every pane, and with a right-aligned prompt that means a redraw at
        // the spawn size before the client's real viewport arrives — which is
        // what leaves zsh's `%` partial-line marker on screen.
        let cmd = if p.cmd.is_empty() {
            vec![self.shell.clone(), "-i".into()]
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

    /// Applies a viewport. Only the owner's counts: a PTY has one size, and two
    /// viewers with different windows would otherwise flap it, repainting the
    /// whole TUI each time.
    pub async fn set_viewport(
        &self,
        grant: &Grant,
        pane: PaneId,
        cols: u16,
        rows: u16,
    ) -> Option<(u16, u16)> {
        if !grant.may_mutate() {
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

    /// Handles one owner-only structural message. Returns the frames to send
    /// back to the caller; the tree broadcast reaches everyone else.
    pub async fn handle_owner(&self, grant: &Grant, msg: In) -> Result<Vec<Out>> {
        if !grant.may_mutate() {
            // Not an error worth telling the client about in detail — a
            // well-behaved client never sends these without `caps.owner`.
            return Ok(Vec::new());
        }

        match msg {
            In::Split { pane, dir } => {
                let new = self.tree.lock().await.split(pane, dir)?;
                self.ensure_running(new).await?;
                self.commit().await;
            }
            In::ClosePane { pane } => {
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
                    Some(t) => tree.active_tab = Some(t),
                    // Activating a workspace without naming a tab must still
                    // land on one of *its* tabs — leaving the previous
                    // workspace's tab id behind meant no tab matched, so the
                    // tab bar showed no selection at all.
                    None => {
                        let stale = tree
                            .active_tab
                            .is_none_or(|t| !tree
                                .workspaces
                                .iter()
                                .find(|w| w.id == ws)
                                .is_some_and(|w| w.tabs.iter().any(|x| x.id == t)));
                        if stale {
                            tree.active_tab = tree
                                .workspaces
                                .iter()
                                .find(|w| w.id == ws)
                                .and_then(|w| w.tabs.first())
                                .map(|t| t.id);
                        }
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
            // Handled by the socket layer, which knows its own session id and
            // so can avoid kicking the connection that asked.
            In::Kick { .. } | In::KickAll => {}
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
            token: "owner".into(),
            scope: Scope::All,
            writable: true,
            pair_hash: None,
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

    pub async fn is_revoked(&self, session: SessionId) -> bool {
        self.store
            .lock()
            .await
            .session_revoked(session)
            .unwrap_or(true)
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
    /// live on as if nothing happened. Disconnect, not kick: these clients
    /// did nothing wrong, and must be able to return when sharing reopens.
    pub async fn set_exposed(&self, on: bool) {
        self.exposed.store(on, std::sync::atomic::Ordering::Relaxed);
        if !on {
            let remote: Vec<SessionId> = self
                .conns
                .lock()
                .await
                .values()
                .filter(|c| {
                    !c.addr
                        .parse::<std::net::IpAddr>()
                        .map(|ip| ip.is_loopback())
                        .unwrap_or(false)
                })
                .map(|c| c.session)
                .collect();
            for s in remote {
                self.disconnect(s).await;
            }
        }
        let _ = self.changed.send(TreeChanged);
    }

    /// Closes one connection without banning it. A kick is punitive and
    /// sticky; this is administrative — used when sharing is switched off.
    pub async fn disconnect(&self, session: SessionId) {
        if let Some(mut conn) = self.conns.lock().await.remove(&session) {
            conn.kick.take();
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
        if self
            .store
            .lock()
            .await
            .put_agent_setting(setting, agent, on)
            .is_err()
        {
            return;
        }
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
        if self.store.lock().await.reset_agent_settings().is_err() {
            return;
        }
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
            if resume_on {
                if let Some(id) = &ev.session_id {
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
            if let Err(e) = self.store.lock().await.save_tree(&tree) {
                tracing::warn!("failed to persist cwd move: {e}");
            }
        }
        let _ = self.agent_tx.send(AgentDelta::Cwd { pane, path: cwd, git });
    }

    /// Registers a live socket and returns its session id. Called on connect;
    /// the id is what a kick names.
    /// Identity of a client across reconnects. Coarse by necessity — there is
    /// no account system — but enough to make a kick stick.
    fn client_key(grant: &Grant, addr: &str, device: &str) -> String {
        format!("{}|{addr}|{device}", grant.token)
    }

    /// True if this client was kicked and must not be let back in.
    pub async fn is_banned(&self, grant: &Grant, addr: &str, device: &str) -> bool {
        // The owner's own window can always reconnect; locking yourself out of
        // your own machine would be absurd.
        if grant.may_mutate() && addr.starts_with("127.") {
            return false;
        }
        self.banned
            .lock()
            .await
            .contains(&Self::client_key(grant, addr, device))
    }

    /// Lets a previously kicked client back in.
    pub async fn unban_all(&self) {
        self.banned.lock().await.clear();
        let _ = self.changed.send(TreeChanged);
    }

    pub async fn add_conn(
        &self,
        grant: &Grant,
        addr: String,
        device: String,
    ) -> (SessionId, tokio::sync::oneshot::Receiver<()>) {
        let session = {
            let mut n = self.next_session.lock().await;
            *n += 1;
            *n
        };
        let (kick_tx, kick_rx) = tokio::sync::oneshot::channel();
        let key = Self::client_key(grant, &addr, &device);
        let conn = Conn {
            session,
            // Self-declared; there is no account system, and the UI says so.
            label: if grant.may_mutate() { "You".into() } else { "Guest".into() },
            device,
            addr,
            scope: Self::session_scope_label(&grant.scope),
            writable: grant.writable,
            since: std::time::Instant::now(),
            kick: Some(kick_tx),
            key,
        };
        self.conns.lock().await.insert(session, conn);
        let _ = self.changed.send(TreeChanged);
        (session, kick_rx)
    }

    /// Closes one connection and stops it coming back.
    pub async fn kick(&self, session: SessionId) -> bool {
        let Some(mut conn) = self.conns.lock().await.remove(&session) else {
            return false;
        };
        self.banned.lock().await.insert(conn.key.clone());
        // Dropping the sender wakes the socket's select arm.
        conn.kick.take();
        let _ = self.changed.send(TreeChanged);
        true
    }

    /// Closes every connection except the one asking.
    pub async fn kick_all_except(&self, keep: SessionId) {
        let victims: Vec<SessionId> = self
            .conns
            .lock()
            .await
            .keys()
            .copied()
            .filter(|s| *s != keep)
            .collect();
        for s in victims {
            self.kick(s).await;
        }
    }

    pub async fn remove_conn(&self, session: SessionId) {
        self.conns.lock().await.remove(&session);
        let _ = self.changed.send(TreeChanged);
    }

    /// Everything currently attached over the network, for the connection
    /// manager. `viewer` is the session asking. Loopback connections are the
    /// owner's own windows — the desktop shell, a local browser tab — and
    /// listing yourself as a "connected client" is noise, so they are
    /// filtered out; only shared (remote) clients appear.
    pub async fn peers(&self, viewer: SessionId) -> Vec<Peer> {
        let mut out: Vec<Peer> = self
            .conns
            .lock()
            .await
            .values()
            .filter(|c| {
                !c.addr
                    .parse::<std::net::IpAddr>()
                    .map(|ip| ip.is_loopback())
                    .unwrap_or(false)
            })
            .map(|c| Peer {
                session: c.session,
                is_you: c.session == viewer,
                label: c.label.clone(),
                device: c.device.clone(),
                addr: c.addr.clone(),
                scope: c.scope.clone(),
                writable: c.writable,
                since_secs: c.since.elapsed().as_secs(),
            })
            .collect();
        out.sort_by_key(|p| p.session);
        out
    }

    /// True while the socket should stay open. A kick revokes the session row
    /// *and* drops it here, so the socket closes instead of lingering.
    pub async fn conn_live(&self, session: SessionId) -> bool {
        self.conns.lock().await.contains_key(&session)
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
    use crate::proto::Dir;
    use crate::share::Scope;

    /// A ready app with one workspace open, as most tests assume. `bootstrap`
    /// no longer opens one itself (a fresh install waits for the user to choose
    /// a folder), so tests open theirs explicitly.
    async fn app() -> Arc<App> {
        let a = App::new(Store::in_memory().unwrap(), 1000);
        a.bootstrap().await.unwrap();
        a.handle_owner(&a.owner_grant().await, In::OpenWorkspace { path: "/tmp".into() })
            .await
            .unwrap();
        a
    }

    fn shared(scope: Scope, writable: bool) -> Grant {
        Grant { token: "t".into(), scope, writable, pair_hash: None }
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

        a.handle_owner(&owner, In::Split { pane, dir: Dir::Vertical })
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
            a.handle_owner(&viewer, msg).await.unwrap();
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
        assert!(a.set_viewport(&viewer, pane, 40, 10).await.is_none());

        // So would a writable one: size is not a write, it is ownership.
        let writable_viewer = shared(Scope::Pane(pane), true);
        assert!(a.set_viewport(&writable_viewer, pane, 40, 10).await.is_none());

        let owner = a.owner_grant().await;
        assert_eq!(a.set_viewport(&owner, pane, 96, 38).await, Some((96, 38)));
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
        assert!(owner_caps.show_sidebar && owner_caps.show_tabs && owner_caps.owner);

        let (view, caps) = a.view_for(&shared(Scope::Pane(pane), false)).await;
        assert!(!caps.show_sidebar, "a pane share must not reveal the sidebar");
        assert!(!caps.show_tabs);
        assert!(!caps.owner && !caps.writable);
        assert_eq!(view.workspaces[0].tabs[0].panes.len(), 1);
    }

    /// The connection-time replay in http.rs skips a barely-started pane for
    /// the owner only (whose browser issued the spawn and would double-print
    /// the prompt), gated on `may_mutate()`. A workspace share — even a
    /// writable one — must therefore report `may_mutate() == false`, or a
    /// viewer switching to a tab the owner never opened gets a blank sheet
    /// because its resumed-but-quiet pane sits under the replay threshold.
    #[tokio::test]
    async fn a_workspace_viewer_is_not_the_owner_and_sees_every_tab() {
        let a = app().await;
        let ws = a.tree.lock().await.workspaces[0].id;

        // A second tab the owner has "opened" but that has produced little —
        // exactly the case that used to replay as blank for a viewer.
        a.handle_owner(&a.owner_grant().await, In::OpenTab { ws })
            .await
            .unwrap();
        assert_eq!(a.tree.lock().await.workspaces[0].tabs.len(), 2);

        for writable in [false, true] {
            let viewer = shared(Scope::Workspace(ws), writable);
            // The replay gate keys on this: a viewer never spawned anything,
            // so it must not inherit the owner's just-spawned skip.
            assert!(
                !viewer.may_mutate(),
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
        a.handle_owner(&a.owner_grant().await, In::Split { pane, dir: Dir::Horizontal })
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
        a.handle_owner(&owner, In::OpenWorkspace { path: "/tmp".into() }).await.unwrap();
        let (ws2, tab2) = {
            let t = a.tree.lock().await;
            (t.workspaces[1].id, t.workspaces[1].tabs[0].id)
        };
        assert_eq!(a.tree.lock().await.active_tab, Some(tab2));

        // Back to workspace 1 by clicking it (no tab named).
        a.handle_owner(&owner, In::Activate { ws: ws1, tab: None }).await.unwrap();
        let t = a.tree.lock().await;
        let active_tab = t.active_tab.expect("a tab must be active");
        assert!(
            t.workspaces[0].tabs.iter().any(|x| x.id == active_tab),
            "the active tab must belong to the activated workspace"
        );
        drop(t);

        // And going forward again keeps ws2's tab.
        a.handle_owner(&owner, In::Activate { ws: ws2, tab: None }).await.unwrap();
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
    async fn closing_the_last_tab_takes_its_workspace_along() {
        // The resume-killer: a tabless workspace survived in the database,
        // bootstrap saw "not empty", and the next launch had no panes at all.
        let a = app().await;
        let owner = a.owner_grant().await;
        let pane = {
            let t = a.tree.lock().await;
            t.workspaces[0].tabs[0].panes[0].id
        };
        a.handle_owner(&owner, In::ClosePane { pane }).await.unwrap();

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
        a.handle_owner(&a.owner_grant().await, In::OpenWorkspace { path: "/keep".into() })
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
        a.handle_owner(&owner, In::OpenTab { ws }).await.unwrap();
        assert_eq!(a.tree.lock().await.workspaces[0].tabs.len(), 2);

        let pane = {
            let t = a.tree.lock().await;
            t.workspaces[0].tabs[1].panes[0].id
        };
        a.handle_owner(&owner, In::ClosePane { pane }).await.unwrap();
        assert_eq!(a.tree.lock().await.workspaces[0].tabs.len(), 1);
    }

    #[tokio::test]
    async fn layout_is_persisted_on_every_change() {
        let a = app().await;
        let pane = {
            let t = a.tree.lock().await;
            t.workspaces[0].tabs[0].panes[0].id
        };
        a.handle_owner(&a.owner_grant().await, In::Split { pane, dir: Dir::Vertical })
            .await
            .unwrap();

        let reloaded = a.store.lock().await.load_tree().unwrap();
        assert_eq!(reloaded.workspaces[0].tabs[0].panes.len(), 2);
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
        a.handle_owner(&a.owner_grant().await, In::Respawn { pane })
            .await
            .unwrap();
        assert!(a.tree.lock().await.pane(pane).unwrap().pty.is_some());
    }
}
