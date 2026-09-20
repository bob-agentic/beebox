//! Share grants and authorisation.
//!
//! Two predicates, and every access decision goes through one of them:
//!
//! * [`visible_panes`] — which panes a grant may see, and therefore which
//!   output it receives and which panes it may type into.
//! * [`Grant::may_mutate`] — whether it may restructure the tree at all.
//!
//! The second exists because half the protocol is not about panes. `OpenTab`,
//! `CloseTab` and `Split` name no pane, so a pane-set check cannot authorise
//! them; without a separate predicate the narrowest share would be able to
//! spawn processes and destroy tabs it cannot even see.

use std::collections::HashSet;

use crate::proto::{GrantScope, Node, PaneId, TabId, WsId};
use crate::session::SessionTree;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The owner's own connection, and the only scope allowed to restructure.
    All,
    Workspace(WsId),
    Tab(TabId),
    Pane(PaneId),
}

impl From<GrantScope> for Scope {
    fn from(g: GrantScope) -> Self {
        match g {
            GrantScope::All => Scope::All,
            GrantScope::Workspace { ws } => Scope::Workspace(ws),
            GrantScope::Tab { tab } => Scope::Tab(tab),
            GrantScope::Pane { pane } => Scope::Pane(pane),
        }
    }
}

impl Scope {
    /// Pairing is forced from Workspace level up: those scopes reveal which
    /// projects and tabs exist, and the project names alone are information.
    pub fn pairing_forced(&self) -> bool {
        matches!(self, Scope::All | Scope::Workspace(_))
    }

    pub fn url_prefix(&self) -> &'static str {
        match self {
            Scope::All => "a",
            Scope::Workspace(_) => "w",
            Scope::Tab(_) => "t",
            Scope::Pane(_) => "p",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Grant {
    pub token: String,
    pub scope: Scope,
    pub writable: bool,
    /// Argon2 hash of the pairing code, cleared once spent.
    pub pair_hash: Option<String>,
}

impl Grant {
    /// Structural changes — spawning panes, opening and closing tabs, resizing
    /// splits, issuing grants, kicking sessions.
    ///
    /// Owner only. A shared link is for showing someone a terminal, not
    /// rearranging the desk; and a remote-supplied command string is a
    /// different risk class from a keystroke.
    pub fn may_mutate(&self) -> bool {
        self.writable && self.scope == Scope::All
    }

    /// Typing into a pane. Requires both the write bit and visibility.
    pub fn may_type(&self, pane: PaneId, tree: &SessionTree) -> bool {
        self.writable && visible_panes(&self.scope, tree).contains(&pane)
    }
}

/// The panes a scope resolves to. Finer scopes yield smaller sets; there is no
/// extra machinery for `Pane` versus `All`.
pub fn visible_panes(scope: &Scope, tree: &SessionTree) -> HashSet<PaneId> {
    let mut out = HashSet::new();
    match *scope {
        Scope::All => {
            for ws in &tree.workspaces {
                for tab in &ws.tabs {
                    collect(&tab.layout, &mut out);
                }
            }
        }
        Scope::Workspace(id) => {
            if let Some(ws) = tree.workspaces.iter().find(|w| w.id == id) {
                for tab in &ws.tabs {
                    collect(&tab.layout, &mut out);
                }
            }
        }
        Scope::Tab(id) => {
            if let Some(tab) = tree.tab(id) {
                collect(&tab.layout, &mut out);
            }
        }
        // Deliberately position-independent: moving the pane to another tab is
        // invisible to its viewer and needs no migration.
        Scope::Pane(id) => {
            if tree.pane(id).is_some() {
                out.insert(id);
            }
        }
    }
    out
}

fn collect(node: &Node, out: &mut HashSet<PaneId>) {
    match node {
        Node::Leaf { pane } => {
            out.insert(*pane);
        }
        Node::Split { children, .. } => {
            for c in children {
                collect(c, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionTree;

    /// Two workspaces; the first has two tabs, one of which is split in two.
    fn fixture() -> SessionTree {
        let mut t = SessionTree::default();
        let ws1 = t.open_workspace("/repo/svc".into(), "svc".into());
        let tab1 = t.open_tab(ws1).unwrap();
        let p1 = t.first_pane(tab1).unwrap();
        t.split(p1, crate::proto::Dir::Vertical).unwrap();
        let _tab2 = t.open_tab(ws1).unwrap();

        let ws2 = t.open_workspace("/repo/ui".into(), "ui".into());
        t.open_tab(ws2).unwrap();
        t
    }

    #[test]
    fn scope_narrows_the_visible_set() {
        let t = fixture();
        let all = visible_panes(&Scope::All, &t);
        let ws = visible_panes(&Scope::Workspace(t.workspaces[0].id), &t);
        let tab = visible_panes(&Scope::Tab(t.workspaces[0].tabs[0].id), &t);

        assert_eq!(all.len(), 4, "2 panes in split tab + 1 + 1");
        assert_eq!(ws.len(), 3);
        assert_eq!(tab.len(), 2, "the split tab holds two panes");
        assert!(ws.is_subset(&all));
        assert!(tab.is_subset(&ws));
    }

    #[test]
    fn pane_scope_survives_the_pane_moving_between_tabs() {
        let t = fixture();
        let pane = t.workspaces[0].tabs[0].panes[0].id;
        assert_eq!(visible_panes(&Scope::Pane(pane), &t).len(), 1);
        // Identity is the pane id, not its position, so a move needs no fixup.
    }

    #[test]
    fn vanished_scope_resolves_to_nothing() {
        let t = fixture();
        assert!(visible_panes(&Scope::Tab(99_999), &t).is_empty());
        assert!(visible_panes(&Scope::Pane(99_999), &t).is_empty());
    }

    /// The hole the review found: a writable Pane grant must not be able to
    /// spawn processes or destroy tabs it cannot see.
    #[test]
    fn only_the_owner_may_mutate_the_tree() {
        let shared = Grant {
            token: "x".into(),
            scope: Scope::Pane(1),
            writable: true,
            pair_hash: None,
        };
        assert!(!shared.may_mutate());

        let owner = Grant { scope: Scope::All, ..shared.clone() };
        assert!(owner.may_mutate());

        let readonly_owner = Grant { writable: false, ..owner.clone() };
        assert!(!readonly_owner.may_mutate());
    }

    #[test]
    fn typing_needs_both_write_and_visibility() {
        let t = fixture();
        let mine = t.workspaces[0].tabs[0].panes[0].id;
        let other = t.workspaces[1].tabs[0].panes[0].id;

        let g = Grant {
            token: "x".into(),
            scope: Scope::Pane(mine),
            writable: true,
            pair_hash: None,
        };
        assert!(g.may_type(mine, &t));
        assert!(!g.may_type(other, &t), "outside the scope");

        let ro = Grant { writable: false, ..g.clone() };
        assert!(!ro.may_type(mine, &t), "read-only cannot type");
    }

    #[test]
    fn pairing_is_forced_from_workspace_level_up() {
        assert!(Scope::All.pairing_forced());
        assert!(Scope::Workspace(1).pairing_forced());
        assert!(!Scope::Tab(1).pairing_forced());
        assert!(!Scope::Pane(1).pairing_forced());
    }
}
