//! The workspace → tab → split tree.
//!
//! Pure structure and geometry. It knows nothing about processes: a pane holds
//! an optional `PtyId`, and the registry owns whatever that points at. Keeping
//! the two apart is what makes sharing a set membership test and restart a
//! swap of one field.

use anyhow::{anyhow, Result};

use crate::agent::AgentState;
use crate::proto::{
    AgentKind, Dir, GitInfo, Node, PaneId, PaneView, PtyId, TabId, TabView, WsId,
};

/// The UI caps nesting here. `Node` stays recursive so lifting the cap is a
/// front-end change, not a migration.
pub const MAX_SPLIT_DEPTH: usize = 2;

/// Spawn size. The VT100 default of 80×24 would make the shell draw its prompt
/// for a screen a quarter the size of a real window, leaving it stranded
/// mid-terminal until the first viewport arrives. These are closer to the
/// common case; the client corrects them immediately.
pub const DEFAULT_COLS: u16 = 120;
pub const DEFAULT_ROWS: u16 = 40;

#[derive(Debug, Clone)]
pub struct Pane {
    pub id: PaneId,
    /// `None` after the process exits — the pane stays so it can be re-run.
    pub pty: Option<PtyId>,
    pub title: String,
    /// Which agent CLI this pane runs, learned from its hooks. `None` for a
    /// plain shell.
    pub agent: Option<AgentKind>,
    /// Hook-driven agent status. Never derived from terminal output or from
    /// the process exit code.
    pub status: AgentState,
    /// The agent session's own title, from structured metadata only.
    pub session_title: Option<String>,
    pub cwd: String,
    pub git: Option<GitInfo>,
    pub cols: u16,
    pub rows: u16,
    /// Argv, kept so a pane can be re-run after its process exits.
    pub cmd: Vec<String>,
    /// The agent's own session id, for `claude --resume` / `codex resume`.
    pub session_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Tab {
    pub id: TabId,
    pub title: String,
    /// Authoritative for structure. Membership is never derived from anywhere
    /// else.
    pub layout: Node,
    pub panes: Vec<Pane>,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: WsId,
    pub name: String,
    pub path: String,
    pub branch: String,
    /// Tabs in the order they were last looked at, most recent first.
    ///
    /// The head is the tab currently in front, so activating this workspace
    /// without naming a tab returns to where you left it — kept per workspace,
    /// because a single shared value cannot say that about several projects.
    ///
    /// The rest is what makes closing a tab land somewhere sensible: you go
    /// back to the one you came from, as a browser does, rather than being
    /// thrown to the first tab from wherever you happened to be.
    ///
    /// Not persisted — after a restart the first tab is the honest answer.
    recent: Vec<TabId>,
    pub tabs: Vec<Tab>,
}

impl Workspace {
    /// Rebuilds a workspace from storage. The visit order is not persisted, so
    /// it starts at the first tab — the only answer a restart can honestly
    /// give. Kept a constructor rather than a public field so the invariant
    /// (head of `recent` is the tab in front) has one owner.
    pub fn restored(id: WsId, name: String, path: String, tabs: Vec<Tab>) -> Self {
        Self {
            id,
            name,
            path,
            branch: String::new(),
            recent: tabs.first().map(|t| t.id).into_iter().collect(),
            tabs,
        }
    }

    /// The tab in front, if it still exists.
    pub fn active_tab(&self) -> Option<TabId> {
        self.recent.first().copied()
    }

    /// Records `tab` as the one in front, keeping the previous order behind it.
    pub fn touch_tab(&mut self, tab: TabId) {
        self.recent.retain(|t| *t != tab);
        self.recent.insert(0, tab);
    }

    /// Discards entries for tabs that no longer exist, so a stale id can never
    /// be handed back as "where to go next".
    fn prune_recent(&mut self) {
        let tabs = &self.tabs;
        self.recent.retain(|id| tabs.iter().any(|t| t.id == *id));
        // A workspace always has somewhere to be, as long as it has tabs.
        if self.recent.is_empty() {
            if let Some(first) = self.tabs.first() {
                self.recent.push(first.id);
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct SessionTree {
    pub workspaces: Vec<Workspace>,
    pub active_ws: Option<WsId>,
    pub active_tab: Option<TabId>,
    next_id: u64,
}

impl SessionTree {
    /// Monotonic and shared across all id kinds: a recycled id would let a
    /// stale grant re-authorise a different terminal.
    fn alloc(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Resumes allocation above everything ever handed out, so a restart
    /// cannot reissue an id that a stale grant still references.
    pub fn set_next_id(&mut self, next: u64) {
        self.next_id = self.next_id.max(next);
    }

    pub fn open_workspace(&mut self, path: String, name: String) -> WsId {
        let id = self.alloc();
        self.workspaces.push(Workspace {
            id,
            name,
            path,
            branch: String::new(),
            recent: Vec::new(),
            tabs: Vec::new(),
        });
        self.active_ws = Some(id);
        id
    }

    pub fn close_workspace(&mut self, ws: WsId) -> Vec<PaneId> {
        let Some(i) = self.workspaces.iter().position(|w| w.id == ws) else {
            return Vec::new();
        };
        let removed = self.workspaces.remove(i);
        if self.active_ws == Some(ws) {
            let next = self.workspaces.first();
            self.active_ws = next.map(|w| w.id);
            // Land on whatever that workspace was last showing, rather than
            // leaving nothing active and making the tab bar pick for us.
            self.active_tab = next.and_then(|w| w.active_tab().or(w.tabs.first().map(|t| t.id)));
        }
        removed
            .tabs
            .iter()
            .flat_map(|t| t.panes.iter().map(|p| p.id))
            .collect()
    }

    pub fn open_tab(&mut self, ws: WsId) -> Result<TabId> {
        let tab_id = self.alloc();
        let pane_id = self.alloc();
        let w = self
            .workspaces
            .iter_mut()
            .find(|w| w.id == ws)
            .ok_or_else(|| anyhow!("no workspace {ws}"))?;

        let pane = Pane {
            id: pane_id,
            pty: None,
            title: String::new(),
            agent: None,
            status: AgentState::default(),
            session_title: None,
            cwd: w.path.clone(),
            git: None,
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            cmd: Vec::new(),
            session_ref: None,
        };
        w.tabs.push(Tab {
            id: tab_id,
            title: String::new(),
            layout: Node::Leaf { pane: pane_id },
            panes: vec![pane],
        });
        w.touch_tab(tab_id);
        self.active_ws = Some(ws);
        self.active_tab = Some(tab_id);
        Ok(tab_id)
    }

    pub fn close_tab(&mut self, tab: TabId) -> Vec<PaneId> {
        for w in &mut self.workspaces {
            if let Some(i) = w.tabs.iter().position(|t| t.id == tab) {
                let removed = w.tabs.remove(i);
                // Dropping the closed tab from the history promotes the one
                // behind it, so you return to where you came from the way a
                // browser does. Falling back to the first tab meant closing
                // tab five threw you to tab one — nowhere you had been.
                w.prune_recent();
                let fallback = w.active_tab();
                if self.active_tab == Some(tab) {
                    self.active_tab = fallback;
                }
                return removed.panes.iter().map(|p| p.id).collect();
            }
        }
        Vec::new()
    }

    /// Splits `pane` in `dir`, returning the new pane. The new pane inherits
    /// cwd so a split lands where you were looking.
    pub fn split(&mut self, pane: PaneId, dir: Dir) -> Result<PaneId> {
        let new_id = self.alloc();
        let tab_id = self
            .tab_of(pane)
            .ok_or_else(|| anyhow!("pane {pane} not in any tab"))?;

        let tab = self.tab_mut(tab_id).expect("just located");
        if depth_of(&tab.layout, pane, 0).is_none_or(|d| d >= MAX_SPLIT_DEPTH) {
            return Err(anyhow!("split depth limit reached"));
        }

        let src = tab
            .panes
            .iter()
            .find(|p| p.id == pane)
            .expect("pane belongs to this tab");
        let new_pane = Pane {
            id: new_id,
            pty: None,
            title: String::new(),
            agent: None,
            status: AgentState::default(),
            session_title: None,
            cwd: src.cwd.clone(),
            git: src.git.clone(),
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            cmd: Vec::new(),
            session_ref: None,
        };
        tab.panes.push(new_pane);
        insert_beside(&mut tab.layout, pane, new_id, dir);
        Ok(new_id)
    }

    /// Removes a pane and collapses any split left holding a single child.
    pub fn close_pane(&mut self, pane: PaneId) -> Option<TabId> {
        let tab_id = self.tab_of(pane)?;
        let tab = self.tab_mut(tab_id)?;

        // Last pane in the tab: the caller closes the tab instead.
        if tab.panes.len() == 1 {
            return None;
        }
        tab.panes.retain(|p| p.id != pane);
        remove_leaf(&mut tab.layout, pane);
        Some(tab_id)
    }

    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.workspaces
            .iter()
            .flat_map(|w| &w.tabs)
            .flat_map(|t| &t.panes)
            .find(|p| p.id == id)
    }

    pub fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.workspaces
            .iter_mut()
            .flat_map(|w| &mut w.tabs)
            .flat_map(|t| &mut t.panes)
            .find(|p| p.id == id)
    }

    pub fn pane_by_pty(&self, pty: PtyId) -> Option<&Pane> {
        self.workspaces
            .iter()
            .flat_map(|w| &w.tabs)
            .flat_map(|t| &t.panes)
            .find(|p| p.pty == Some(pty))
    }

    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.workspaces.iter().flat_map(|w| &w.tabs).find(|t| t.id == id)
    }

    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.workspaces
            .iter_mut()
            .flat_map(|w| &mut w.tabs)
            .find(|t| t.id == id)
    }

    pub fn tab_of(&self, pane: PaneId) -> Option<TabId> {
        self.workspaces
            .iter()
            .flat_map(|w| &w.tabs)
            .find(|t| t.panes.iter().any(|p| p.id == pane))
            .map(|t| t.id)
    }

    pub fn first_pane(&self, tab: TabId) -> Option<PaneId> {
        self.tab(tab)?.panes.first().map(|p| p.id)
    }

    /// Applies a gutter drag. `path` indexes into the split tree from the
    /// root, so the front end can address a nested split without naming panes.
    pub fn set_sizes(&mut self, tab: TabId, path: &[u32], sizes: &[f32]) -> Result<()> {
        let node = self
            .tab_mut(tab)
            .ok_or_else(|| anyhow!("no tab {tab}"))?
            .layout
            .at_mut(path)
            .ok_or_else(|| anyhow!("no split at {path:?}"))?;

        let Node::Split { children, sizes: s, .. } = node else {
            return Err(anyhow!("not a split"));
        };
        if sizes.len() != children.len() {
            return Err(anyhow!(
                "expected {} sizes, got {}",
                children.len(),
                sizes.len()
            ));
        }
        // Normalise so rounding on the client cannot drift the total.
        let total: f32 = sizes.iter().sum();
        if total <= 0.0 {
            return Err(anyhow!("sizes must be positive"));
        }
        *s = sizes.iter().map(|v| v / total).collect();
        Ok(())
    }

    pub fn to_view(&self, visible: &std::collections::HashSet<PaneId>) -> crate::proto::TreeView {
        let workspaces = self
            .workspaces
            .iter()
            .filter_map(|w| {
                let tabs: Vec<TabView> = w
                    .tabs
                    .iter()
                    .filter_map(|t| {
                        let panes: Vec<PaneView> = t
                            .panes
                            .iter()
                            .filter(|p| visible.contains(&p.id))
                            .map(|p| PaneView {
                                id: p.id,
                                pty: p.pty,
                                title: p.title.clone(),
                                agent: p.agent,
                                status: p.status.view().clone(),
                                session_title: p.session_title.clone(),
                                cwd: p.cwd.clone(),
                                git: p.git.clone(),
                                cols: p.cols,
                                rows: p.rows,
                            })
                            .collect();
                        if panes.is_empty() {
                            return None;
                        }
                        // Prune the layout to the visible leaves so a scoped
                        // viewer never learns of panes outside its grant.
                        let layout = prune(&t.layout, visible)?;
                        Some(TabView {
                            id: t.id,
                            title: t.title.clone(),
                            layout,
                            panes,
                        })
                    })
                    .collect();
                if tabs.is_empty() {
                    return None;
                }
                Some(crate::proto::WorkspaceView {
                    id: w.id,
                    name: w.name.clone(),
                    path: w.path.clone(),
                    branch: w.branch.clone(),
                    tabs,
                })
            })
            .collect();

        crate::proto::TreeView {
            workspaces,
            active_ws: self.active_ws,
            active_tab: self.active_tab,
        }
    }
}

impl Node {
    /// Walks `path` from this node, where each element is a child index.
    fn at_mut(&mut self, path: &[u32]) -> Option<&mut Node> {
        match path.split_first() {
            None => Some(self),
            Some((i, rest)) => match self {
                Node::Split { children, .. } => {
                    children.get_mut(*i as usize)?.at_mut(rest)
                }
                Node::Leaf { .. } => None,
            },
        }
    }
}

fn depth_of(node: &Node, pane: PaneId, d: usize) -> Option<usize> {
    match node {
        Node::Leaf { pane: p } => (*p == pane).then_some(d),
        Node::Split { children, .. } => children.iter().find_map(|c| depth_of(c, pane, d + 1)),
    }
}

/// Puts `new_pane` next to `target`. If the enclosing split already runs in
/// `dir`, it joins that split rather than nesting a new one — matching how
/// iTerm2 behaves and keeping depth down.
fn insert_beside(node: &mut Node, target: PaneId, new_pane: PaneId, dir: Dir) {
    if let Node::Split { dir: d, children, sizes } = node {
        if *d == dir {
            if let Some(i) = children.iter().position(
                |c| matches!(c, Node::Leaf { pane } if *pane == target),
            ) {
                children.insert(i + 1, Node::Leaf { pane: new_pane });
                let even = 1.0 / children.len() as f32;
                *sizes = vec![even; children.len()];
                return;
            }
        }
        for c in children.iter_mut() {
            insert_beside(c, target, new_pane, dir);
        }
        return;
    }

    if matches!(node, Node::Leaf { pane } if *pane == target) {
        *node = Node::Split {
            dir,
            children: vec![Node::Leaf { pane: target }, Node::Leaf { pane: new_pane }],
            sizes: vec![0.5, 0.5],
        };
    }
}

fn remove_leaf(node: &mut Node, pane: PaneId) {
    if let Node::Split { children, sizes, .. } = node {
        if let Some(i) = children
            .iter()
            .position(|c| matches!(c, Node::Leaf { pane: p } if *p == pane))
        {
            children.remove(i);
            sizes.remove(i);
        } else {
            for c in children.iter_mut() {
                remove_leaf(c, pane);
            }
        }

        // A split with one child is not a split.
        if children.len() == 1 {
            *node = children.remove(0);
        } else {
            let even = 1.0 / children.len() as f32;
            *sizes = vec![even; children.len()];
        }
    }
}

/// Drops leaves outside `visible`, collapsing splits that lose all but one
/// child. Returns `None` if nothing remains.
fn prune(node: &Node, visible: &std::collections::HashSet<PaneId>) -> Option<Node> {
    match node {
        Node::Leaf { pane } => visible.contains(pane).then(|| node.clone()),
        Node::Split { dir, children, sizes } => {
            let kept: Vec<(Node, f32)> = children
                .iter()
                .zip(sizes)
                .filter_map(|(c, s)| prune(c, visible).map(|n| (n, *s)))
                .collect();
            match kept.len() {
                0 => None,
                1 => Some(kept.into_iter().next().unwrap().0),
                _ => {
                    let total: f32 = kept.iter().map(|(_, s)| s).sum();
                    Some(Node::Split {
                        dir: *dir,
                        children: kept.iter().map(|(n, _)| n.clone()).collect(),
                        sizes: kept.iter().map(|(_, s)| s / total).collect(),
                    })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_with_one_pane() -> (SessionTree, TabId, PaneId) {
        let mut t = SessionTree::default();
        let ws = t.open_workspace("/repo/svc".into(), "svc".into());
        let tab = t.open_tab(ws).unwrap();
        let pane = t.first_pane(tab).unwrap();
        (t, tab, pane)
    }

    #[test]
    fn split_produces_the_prototype_layout() {
        // One pane beside a stack of two — proto/index.html's stage.
        let (mut t, tab, left) = tab_with_one_pane();
        let right = t.split(left, Dir::Vertical).unwrap();
        let bottom = t.split(right, Dir::Horizontal).unwrap();

        let layout = &t.tab(tab).unwrap().layout;
        let Node::Split { dir, children, .. } = layout else { panic!("expected split") };
        assert_eq!(*dir, Dir::Vertical);
        assert!(matches!(children[0], Node::Leaf { pane } if pane == left));

        let Node::Split { dir, children, .. } = &children[1] else { panic!("expected nested") };
        assert_eq!(*dir, Dir::Horizontal);
        assert_eq!(children.len(), 2);
        assert!(matches!(children[1], Node::Leaf { pane } if pane == bottom));
    }

    #[test]
    fn same_direction_split_widens_instead_of_nesting() {
        let (mut t, tab, a) = tab_with_one_pane();
        let b = t.split(a, Dir::Vertical).unwrap();
        let _c = t.split(b, Dir::Vertical).unwrap();

        let Node::Split { children, sizes, .. } = &t.tab(tab).unwrap().layout else {
            panic!("expected split")
        };
        assert_eq!(children.len(), 3, "joined the existing split");
        assert!(sizes.iter().all(|s| (*s - 1.0 / 3.0).abs() < 1e-6));
    }

    #[test]
    fn depth_limit_is_enforced() {
        let (mut t, _tab, a) = tab_with_one_pane();
        let b = t.split(a, Dir::Vertical).unwrap();
        let c = t.split(b, Dir::Horizontal).unwrap();
        assert!(t.split(c, Dir::Vertical).is_err(), "third level rejected");
    }

    #[test]
    fn closing_a_pane_collapses_the_orphaned_split() {
        let (mut t, tab, a) = tab_with_one_pane();
        let b = t.split(a, Dir::Vertical).unwrap();
        t.close_pane(b);

        let layout = &t.tab(tab).unwrap().layout;
        assert!(matches!(layout, Node::Leaf { pane } if *pane == a));
        assert_eq!(t.tab(tab).unwrap().panes.len(), 1);
    }

    #[test]
    fn closing_the_last_pane_is_refused() {
        let (mut t, _tab, a) = tab_with_one_pane();
        assert!(t.close_pane(a).is_none(), "caller closes the tab instead");
    }

    #[test]
    fn ids_are_never_reused() {
        let (mut t, tab, a) = tab_with_one_pane();
        let b = t.split(a, Dir::Vertical).unwrap();
        t.close_pane(b);
        let c = t.split(a, Dir::Vertical).unwrap();
        assert_ne!(b, c, "a recycled id would re-authorise a stale grant");
        let _ = tab;
    }

    #[test]
    fn view_prunes_layout_to_the_visible_pane() {
        let (mut t, tab, a) = tab_with_one_pane();
        let b = t.split(a, Dir::Vertical).unwrap();

        let only_b = std::collections::HashSet::from([b]);
        let view = t.to_view(&only_b);

        let tv = &view.workspaces[0].tabs[0];
        assert_eq!(tv.panes.len(), 1);
        assert!(
            matches!(tv.layout, Node::Leaf { pane } if pane == b),
            "the split collapsed; the viewer never learns pane {a} exists"
        );
        let _ = tab;
    }

    #[test]
    fn view_hides_workspaces_with_nothing_visible() {
        let mut t = SessionTree::default();
        let ws1 = t.open_workspace("/a".into(), "a".into());
        let tab1 = t.open_tab(ws1).unwrap();
        let ws2 = t.open_workspace("/b".into(), "b".into());
        t.open_tab(ws2).unwrap();

        let visible = std::collections::HashSet::from([t.first_pane(tab1).unwrap()]);
        let view = t.to_view(&visible);
        assert_eq!(view.workspaces.len(), 1);
        assert_eq!(view.workspaces[0].name, "a");
    }
}
