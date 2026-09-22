//! SQLite persistence. Stores only what cannot be recovered: which workspaces
//! and tabs exist, how their panes are arranged, and who has been granted
//! access.
//!
//! Scrollback is deliberately absent — it lives in the ring and dies with the
//! process.

use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::proto::{Node, TabId, WsId};
use crate::session::{Pane, SessionTree, Tab, Workspace};
use crate::share::{Grant, Scope};

pub struct Store {
    db: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let db = Connection::open(path)?;
        Self::init(&db)?;
        Ok(Self { db })
    }

    pub fn in_memory() -> Result<Self> {
        let db = Connection::open_in_memory()?;
        Self::init(&db)?;
        Ok(Self { db })
    }

    fn init(db: &Connection) -> Result<()> {
        db.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS workspaces (
                id    INTEGER PRIMARY KEY,
                name  TEXT NOT NULL,
                path  TEXT NOT NULL,
                ord   INTEGER NOT NULL
            );

            -- layout_json is authoritative for structure. Pane membership is
            -- never derived from panes.tab_id; two sources of truth for the
            -- same fact would drift.
            CREATE TABLE IF NOT EXISTS tabs (
                id          INTEGER PRIMARY KEY,
                ws_id       INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
                title       TEXT NOT NULL,
                ord         INTEGER NOT NULL,
                layout_json TEXT NOT NULL
            );

            -- Authoritative for per-pane attributes only. `agent` holds the
            -- protocol enum string ("claude"/"codex"/"opencode") or '' for a
            -- plain shell; UI badges are derived client-side.
            CREATE TABLE IF NOT EXISTS panes (
                id          INTEGER PRIMARY KEY,
                tab_id      INTEGER NOT NULL REFERENCES tabs(id) ON DELETE CASCADE,
                cmd         TEXT NOT NULL,
                cwd         TEXT NOT NULL,
                agent       TEXT NOT NULL,
                session_ref TEXT
            );
            -- cols/rows are added by the migration below rather than here, so
            -- that new and upgraded databases end up with one shape.

            CREATE TABLE IF NOT EXISTS grants (
                token      TEXT PRIMARY KEY,
                scope_kind TEXT NOT NULL,
                scope_id   INTEGER,
                writable   INTEGER NOT NULL,
                pair_hash  TEXT,
                created    INTEGER NOT NULL
            );

            -- Kick revokes the session row, not just the socket: closing only
            -- the socket would let a reload restore access.
            CREATE TABLE IF NOT EXISTS sessions (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                token     TEXT NOT NULL REFERENCES grants(token) ON DELETE CASCADE,
                label     TEXT NOT NULL,
                device    TEXT NOT NULL,
                addr      TEXT NOT NULL,
                created   INTEGER NOT NULL,
                last_seen INTEGER NOT NULL,
                revoked   INTEGER NOT NULL DEFAULT 0
            );

            -- Monotonic id source, shared by panes/tabs/workspaces so an id is
            -- never reused: a recycled id would let a stale grant re-authorise
            -- a different terminal.
            CREATE TABLE IF NOT EXISTS id_seq (
                next INTEGER NOT NULL
            );
            INSERT INTO id_seq (next)
                SELECT 1 WHERE NOT EXISTS (SELECT 1 FROM id_seq);

            -- Daemon-owned settings (the Agents toggles). Missing key = false.
            CREATE TABLE IF NOT EXISTS settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;

        // M3 adds the agent session title. ALTER-if-missing keeps databases
        // written by earlier builds loading without a version table.
        let has_session_title = db
            .prepare("SELECT 1 FROM pragma_table_info('panes') WHERE name = 'session_title'")?
            .exists([])?;
        if !has_session_title {
            db.execute("ALTER TABLE panes ADD COLUMN session_title TEXT", [])?;
        }

        // Viewport size, so a restart respawns at the size the pane actually
        // had. It used to be restored as a hardcoded 80x24, and a shell drawing
        // its first prompt at a width the terminal does not have leaves zsh's
        // reverse-video `%` behind — the marker is erased by padding to an
        // exact column count, so the wrong count erases nothing.
        let has_cols = db
            .prepare("SELECT 1 FROM pragma_table_info('panes') WHERE name = 'cols'")?
            .exists([])?;
        if !has_cols {
            // Defaulted, not nullable: old rows then read back as the same
            // guess they were already getting, with no Option to unwrap.
            db.execute("ALTER TABLE panes ADD COLUMN cols INTEGER NOT NULL DEFAULT 80", [])?;
            db.execute("ALTER TABLE panes ADD COLUMN rows INTEGER NOT NULL DEFAULT 24", [])?;
        }
        Ok(())
    }

    /// Reserves `n` ids, so the in-memory tree and the database agree on what
    /// has ever been handed out.
    pub fn reserve_ids(&self, n: u64) -> Result<u64> {
        let tx = self.db.unchecked_transaction()?;
        let from = from_db(tx.query_row("SELECT next FROM id_seq", [], |r| r.get::<_, i64>(0))?);
        tx.execute("UPDATE id_seq SET next = ?1", params![to_db(from + n)])?;
        tx.commit()?;
        Ok(from)
    }

    /// Replaces the persisted layout with the current tree. Whole-tree rewrite
    /// is the right call at this scale: a few dozen rows, and no chance of the
    /// two drifting.
    pub fn save_tree(&self, tree: &SessionTree) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute("DELETE FROM workspaces", [])?;

        for (wi, ws) in tree.workspaces.iter().enumerate() {
            tx.execute(
                "INSERT INTO workspaces (id, name, path, ord) VALUES (?1, ?2, ?3, ?4)",
                params![to_db(ws.id), ws.name, ws.path, wi as i64],
            )?;
            for (ti, tab) in ws.tabs.iter().enumerate() {
                tx.execute(
                    "INSERT INTO tabs (id, ws_id, title, ord, layout_json)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        to_db(tab.id),
                        to_db(ws.id),
                        tab.title,
                        ti as i64,
                        serde_json::to_string(&tab.layout)?
                    ],
                )?;
                for pane in &tab.panes {
                    tx.execute(
                        "INSERT INTO panes (id, tab_id, cmd, cwd, agent, session_ref, session_title, cols, rows)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        params![
                            to_db(pane.id),
                            to_db(tab.id),
                            pane.cmd.join(" "),
                            pane.cwd,
                            pane.agent.map(crate::agent::agent_kind_str).unwrap_or(""),
                            pane.session_ref,
                            pane.session_title,
                            pane.cols,
                            pane.rows
                        ],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Restores the layout. Panes come back with no `pty`: nothing is running
    /// yet, and the pane exists so it can be re-run.
    pub fn load_tree(&self) -> Result<SessionTree> {
        let mut tree = SessionTree::default();

        let mut ws_stmt = self
            .db
            .prepare("SELECT id, name, path FROM workspaces ORDER BY ord")?;
        let workspaces: Vec<(WsId, String, String)> = ws_stmt
            .query_map([], |r| Ok((from_db(r.get(0)?), r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;

        for (ws_id, name, path) in workspaces {
            let mut tabs = Vec::new();

            let mut tab_stmt = self.db.prepare(
                "SELECT id, title, layout_json FROM tabs WHERE ws_id = ?1 ORDER BY ord",
            )?;
            let rows: Vec<(TabId, String, String)> = tab_stmt
                .query_map(params![to_db(ws_id)], |r| {
                    Ok((from_db(r.get(0)?), r.get(1)?, r.get(2)?))
                })?
                .collect::<rusqlite::Result<_>>()?;

            for (tab_id, title, layout_json) in rows {
                let layout: Node = serde_json::from_str(&layout_json)?;

                let mut pane_stmt = self.db.prepare(
                    "SELECT id, cmd, cwd, agent, session_ref, session_title, cols, rows
                     FROM panes WHERE tab_id = ?1",
                )?;
                let panes: Vec<Pane> = pane_stmt
                    .query_map(params![to_db(tab_id)], |r| {
                        let cmd: String = r.get(1)?;
                        let agent: String = r.get(3)?;
                        Ok(Pane {
                            id: from_db(r.get(0)?),
                            pty: None,
                            title: String::new(),
                            // Legacy rows hold display strings ("SH"/"CC");
                            // parse_agent_kind maps those to None, which is
                            // the correct reading for a pane whose agent was
                            // never confirmed by a hook.
                            agent: crate::agent::parse_agent_kind(&agent),
                            status: crate::agent::AgentState::default(),
                            session_title: r.get(5)?,
                            cwd: r.get(2)?,
                            git: None,
                            // The size this pane actually had. Restoring a
                            // guess here is what made a restored shell draw
                            // its prompt at the wrong width.
                            cols: r.get(6)?,
                            rows: r.get(7)?,
                            cmd: cmd.split_whitespace().map(str::to_string).collect(),
                            session_ref: r.get(4)?,
                        })
                    })?
                    .collect::<rusqlite::Result<_>>()?;

                tabs.push(Tab { id: tab_id, title, layout, panes });
            }

            // Which tab was in front, and the order tabs were visited in, are
            // not persisted; `restored` starts at the first tab, which is the
            // only answer a restart can honestly give.
            tree.workspaces.push(Workspace::restored(ws_id, name, path, tabs));
        }

        // Resume id allocation above everything ever handed out.
        let next = from_db(
            self.db
                .query_row("SELECT next FROM id_seq", [], |r| r.get::<_, i64>(0))?,
        );
        tree.set_next_id(next);
        tree.active_ws = tree.workspaces.first().map(|w| w.id);
        tree.active_tab = tree
            .workspaces
            .first()
            .and_then(|w| w.tabs.first())
            .map(|t| t.id);
        Ok(tree)
    }

    pub fn put_grant(&self, g: &Grant) -> Result<()> {
        let (kind, id) = scope_parts(&g.scope);
        self.db.execute(
            "INSERT OR REPLACE INTO grants
             (token, scope_kind, scope_id, writable, pair_hash, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![g.token, kind, id.map(to_db), g.writable as i64, g.pair_hash, now()],
        )?;
        Ok(())
    }

    pub fn grant(&self, token: &str) -> Result<Option<Grant>> {
        let mut stmt = self.db.prepare(
            "SELECT token, scope_kind, scope_id, writable, pair_hash
             FROM grants WHERE token = ?1",
        )?;
        let mut rows = stmt.query(params![token])?;
        let Some(r) = rows.next()? else { return Ok(None) };

        let kind: String = r.get(1)?;
        let id: Option<i64> = r.get(2)?;
        let id = id.map(from_db);
        Ok(Some(Grant {
            token: r.get(0)?,
            scope: scope_from(&kind, id),
            writable: r.get::<_, i64>(3)? != 0,
            pair_hash: r.get(4)?,
        }))
    }

    pub fn delete_grant(&self, token: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM grants WHERE token = ?1", params![token])?;
        Ok(())
    }

    /// Clears the pairing requirement once a code has been spent.
    pub fn clear_pairing(&self, token: &str) -> Result<()> {
        self.db.execute(
            "UPDATE grants SET pair_hash = NULL WHERE token = ?1",
            params![token],
        )?;
        Ok(())
    }

    pub fn open_session(
        &self,
        token: &str,
        label: &str,
        device: &str,
        addr: &str,
    ) -> Result<u64> {
        let t = now();
        self.db.execute(
            "INSERT INTO sessions (token, label, device, addr, created, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![token, label, device, addr, t],
        )?;
        Ok(from_db(self.db.last_insert_rowid()))
    }

    pub fn revoke_session(&self, id: u64) -> Result<()> {
        self.db
            .execute("UPDATE sessions SET revoked = 1 WHERE id = ?1", params![to_db(id)])?;
        Ok(())
    }

    pub fn revoke_all_sessions(&self) -> Result<()> {
        self.db.execute("UPDATE sessions SET revoked = 1", [])?;
        Ok(())
    }

    pub fn session_revoked(&self, id: u64) -> Result<bool> {
        Ok(self
            .db
            .query_row(
                "SELECT revoked FROM sessions WHERE id = ?1",
                params![to_db(id)],
                |r| r.get::<_, i64>(0),
            )
            .map(|v| v != 0)
            .unwrap_or(true))
    }

    pub fn touch_session(&self, id: u64) -> Result<()> {
        self.db.execute(
            "UPDATE sessions SET last_seen = ?2 WHERE id = ?1",
            params![to_db(id), now()],
        )?;
        Ok(())
    }

    // ---- agent settings -----------------------------------------------

    /// Keys follow the handover doc: `agent.status.claude`, `agent.resume.codex`, …
    fn setting_key(setting: crate::proto::AgentSetting, agent: crate::proto::AgentKind) -> String {
        let s = match setting {
            crate::proto::AgentSetting::Status => "status",
            crate::proto::AgentSetting::Resume => "resume",
        };
        format!("agent.{s}.{}", crate::agent::agent_kind_str(agent))
    }

    pub fn load_agent_settings(&self) -> Result<crate::proto::AgentSettings> {
        use crate::proto::{AgentKind, AgentSetting};
        let mut out = crate::proto::AgentSettings::default();
        let mut stmt = self
            .db
            .prepare("SELECT key, value FROM settings WHERE key LIKE 'agent.%'")?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (k, v) in rows {
            let on = v == "true";
            for agent in [AgentKind::Claude, AgentKind::Opencode, AgentKind::Codex] {
                for setting in [AgentSetting::Status, AgentSetting::Resume] {
                    if k == Self::setting_key(setting, agent) {
                        out.set(agent, setting, on);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Writes one toggle. When a resume toggle goes OFF, the same transaction
    /// clears every stored session id for that agent — a later crash cannot
    /// leave ids behind that the setting says must not exist.
    pub fn put_agent_setting(
        &self,
        setting: crate::proto::AgentSetting,
        agent: crate::proto::AgentKind,
        on: bool,
    ) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            params![Self::setting_key(setting, agent), if on { "true" } else { "false" }],
        )?;
        if setting == crate::proto::AgentSetting::Resume && !on {
            tx.execute(
                "UPDATE panes SET session_ref = NULL WHERE agent = ?1",
                params![crate::agent::agent_kind_str(agent)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The Reset button: all six off, every stored resume id gone.
    pub fn reset_agent_settings(&self) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute("DELETE FROM settings WHERE key LIKE 'agent.%'", [])?;
        tx.execute("UPDATE panes SET session_ref = NULL", [])?;
        tx.commit()?;
        Ok(())
    }
}

fn scope_parts(s: &Scope) -> (&'static str, Option<u64>) {
    match *s {
        Scope::All => ("all", None),
        Scope::Workspace(id) => ("ws", Some(id)),
        Scope::Tab(id) => ("tab", Some(id)),
        Scope::Pane(id) => ("pane", Some(id)),
    }
}

fn scope_from(kind: &str, id: Option<u64>) -> Scope {
    match (kind, id) {
        ("ws", Some(id)) => Scope::Workspace(id),
        ("tab", Some(id)) => Scope::Tab(id),
        ("pane", Some(id)) => Scope::Pane(id),
        _ => Scope::All,
    }
}

/// SQLite has no unsigned 64-bit type, so ids cross the boundary as `i64`.
/// Our ids are monotonic from 1 and will never approach the sign bit.
fn to_db(id: u64) -> i64 {
    id as i64
}

fn from_db(id: i64) -> u64 {
    id as u64
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::Dir;

    fn tree_with_split() -> SessionTree {
        let mut t = SessionTree::default();
        let ws = t.open_workspace("/repo/svc".into(), "svc".into());
        let tab = t.open_tab(ws).unwrap();
        let a = t.first_pane(tab).unwrap();
        let b = t.split(a, Dir::Vertical).unwrap();
        t.split(b, Dir::Horizontal).unwrap();
        t
    }

    #[test]
    fn layout_survives_a_restart() {
        let s = Store::in_memory().unwrap();
        let before = tree_with_split();
        s.save_tree(&before).unwrap();

        let after = s.load_tree().unwrap();
        assert_eq!(after.workspaces.len(), 1);
        assert_eq!(after.workspaces[0].name, "svc");

        let tab = &after.workspaces[0].tabs[0];
        assert_eq!(tab.panes.len(), 3);
        // The nested split geometry must come back intact.
        let Node::Split { children, .. } = &tab.layout else { panic!("lost the split") };
        assert!(matches!(children[1], Node::Split { .. }));
    }

    #[test]
    fn restored_panes_have_no_process() {
        let s = Store::in_memory().unwrap();
        s.save_tree(&tree_with_split()).unwrap();
        let after = s.load_tree().unwrap();
        // A pane exists so it can be re-run; nothing is running at load time.
        assert!(after.workspaces[0].tabs[0].panes.iter().all(|p| p.pty.is_none()));
    }

    #[test]
    fn ids_are_not_reused_across_restarts() {
        let s = Store::in_memory().unwrap();
        let before = tree_with_split();
        let highest = before
            .workspaces
            .iter()
            .flat_map(|w| &w.tabs)
            .flat_map(|t| &t.panes)
            .map(|p| p.id)
            .max()
            .unwrap();
        s.reserve_ids(highest + 1).unwrap();
        s.save_tree(&before).unwrap();

        let mut after = s.load_tree().unwrap();
        let tab = after.workspaces[0].tabs[0].id;
        let pane = after.first_pane(tab).unwrap();
        let fresh = after.split(pane, Dir::Vertical);
        // Depth may refuse the split; what matters is the id, when granted.
        if let Ok(id) = fresh {
            assert!(id > highest, "id {id} collides with pre-restart ids");
        }
    }

    #[test]
    fn session_ref_round_trips_for_auto_resume() {
        let s = Store::in_memory().unwrap();
        let mut t = SessionTree::default();
        let ws = t.open_workspace("/repo/svc".into(), "svc".into());
        let tab = t.open_tab(ws).unwrap();
        let pane = t.first_pane(tab).unwrap();
        t.pane_mut(pane).unwrap().session_ref = Some("abc-123".into());
        t.pane_mut(pane).unwrap().cmd = vec!["claude".into()];
        s.save_tree(&t).unwrap();

        let after = s.load_tree().unwrap();
        let p = &after.workspaces[0].tabs[0].panes[0];
        assert_eq!(p.session_ref.as_deref(), Some("abc-123"));
        assert_eq!(p.cmd, vec!["claude"]);
    }

    #[test]
    fn grants_round_trip_every_scope() {
        let s = Store::in_memory().unwrap();
        for scope in [Scope::All, Scope::Workspace(2), Scope::Tab(3), Scope::Pane(4)] {
            let g = Grant {
                token: format!("tok{:?}", scope),
                scope,
                writable: true,
                pair_hash: Some("hash".into()),
            };
            s.put_grant(&g).unwrap();
            let back = s.grant(&g.token).unwrap().expect("grant missing");
            assert_eq!(back.scope, scope);
            assert!(back.writable);
        }
    }

    #[test]
    fn spent_pairing_is_cleared() {
        let s = Store::in_memory().unwrap();
        let g = Grant {
            token: "t".into(),
            scope: Scope::Pane(1),
            writable: false,
            pair_hash: Some("hash".into()),
        };
        s.put_grant(&g).unwrap();
        s.clear_pairing("t").unwrap();
        assert!(s.grant("t").unwrap().unwrap().pair_hash.is_none());
    }

    #[test]
    fn kick_revokes_the_session_not_just_the_socket() {
        let s = Store::in_memory().unwrap();
        s.put_grant(&Grant {
            token: "t".into(),
            scope: Scope::All,
            writable: true,
            pair_hash: None,
        })
        .unwrap();

        let id = s.open_session("t", "Zhang", "Chrome/macOS", "100.64.0.87").unwrap();
        assert!(!s.session_revoked(id).unwrap());

        s.revoke_session(id).unwrap();
        // Reloading the page must not restore access.
        assert!(s.session_revoked(id).unwrap());
    }

    #[test]
    fn unknown_session_counts_as_revoked() {
        let s = Store::in_memory().unwrap();
        assert!(s.session_revoked(999).unwrap(), "fail closed");
    }

    #[test]
    fn deleting_a_grant_drops_its_sessions() {
        let s = Store::in_memory().unwrap();
        s.put_grant(&Grant {
            token: "t".into(),
            scope: Scope::All,
            writable: true,
            pair_hash: None,
        })
        .unwrap();
        let id = s.open_session("t", "a", "b", "c").unwrap();
        s.delete_grant("t").unwrap();
        assert!(s.session_revoked(id).unwrap(), "cascade must revoke access");
    }

    #[test]
    fn agent_settings_default_off_and_toggle_independently() {
        use crate::proto::{AgentKind, AgentSetting};
        let s = Store::in_memory().unwrap();
        let d = s.load_agent_settings().unwrap();
        assert_eq!(d, crate::proto::AgentSettings::default(), "all six default OFF");

        s.put_agent_setting(AgentSetting::Status, AgentKind::Claude, true).unwrap();
        s.put_agent_setting(AgentSetting::Resume, AgentKind::Codex, true).unwrap();
        let l = s.load_agent_settings().unwrap();
        assert!(l.status_claude && l.resume_codex);
        assert!(!l.status_codex && !l.status_opencode && !l.resume_claude && !l.resume_opencode);
    }

    #[test]
    fn resume_off_clears_that_agents_session_refs_only() {
        use crate::proto::{AgentKind, AgentSetting};
        let s = Store::in_memory().unwrap();
        let mut t = SessionTree::default();
        let ws = t.open_workspace("/r".into(), "r".into());
        let tab = t.open_tab(ws).unwrap();
        let a = t.first_pane(tab).unwrap();
        let b = t.split(a, Dir::Vertical).unwrap();
        {
            let p = t.pane_mut(a).unwrap();
            p.agent = Some(AgentKind::Claude);
            p.session_ref = Some("claude-id".into());
        }
        {
            let p = t.pane_mut(b).unwrap();
            p.agent = Some(AgentKind::Codex);
            p.session_ref = Some("codex-id".into());
        }
        s.save_tree(&t).unwrap();

        s.put_agent_setting(AgentSetting::Resume, AgentKind::Claude, false).unwrap();
        let after = s.load_tree().unwrap();
        let panes = &after.workspaces[0].tabs[0].panes;
        let by_id = |id: u64| panes.iter().find(|p| p.id == id).unwrap();
        assert!(by_id(a).session_ref.is_none(), "claude id cleared");
        assert_eq!(by_id(b).session_ref.as_deref(), Some("codex-id"), "codex untouched");
    }

    #[test]
    fn reset_clears_all_settings_and_all_session_refs() {
        use crate::proto::{AgentKind, AgentSetting};
        let s = Store::in_memory().unwrap();
        s.put_agent_setting(AgentSetting::Status, AgentKind::Claude, true).unwrap();
        s.put_agent_setting(AgentSetting::Resume, AgentKind::Codex, true).unwrap();

        let mut t = SessionTree::default();
        let ws = t.open_workspace("/r".into(), "r".into());
        let tab = t.open_tab(ws).unwrap();
        let pane = t.first_pane(tab).unwrap();
        t.pane_mut(pane).unwrap().agent = Some(AgentKind::Claude);
        t.pane_mut(pane).unwrap().session_ref = Some("sid".into());
        s.save_tree(&t).unwrap();

        s.reset_agent_settings().unwrap();
        assert_eq!(s.load_agent_settings().unwrap(), crate::proto::AgentSettings::default());
        assert!(s.load_tree().unwrap().workspaces[0].tabs[0].panes[0].session_ref.is_none());
    }

    #[test]
    fn a_pre_m3_database_still_loads() {
        // Simulate a database created before the session_title column and the
        // settings table existed, with the old display-string agent values.
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            r#"
            CREATE TABLE workspaces (id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                path TEXT NOT NULL, ord INTEGER NOT NULL);
            CREATE TABLE tabs (id INTEGER PRIMARY KEY, ws_id INTEGER NOT NULL,
                title TEXT NOT NULL, ord INTEGER NOT NULL, layout_json TEXT NOT NULL);
            CREATE TABLE panes (id INTEGER PRIMARY KEY, tab_id INTEGER NOT NULL,
                cmd TEXT NOT NULL, cwd TEXT NOT NULL, agent TEXT NOT NULL,
                session_ref TEXT);
            INSERT INTO workspaces VALUES (1, 'w', '/w', 0);
            INSERT INTO tabs VALUES (2, 1, '', 0, '{"kind":"leaf","pane":3}');
            INSERT INTO panes VALUES (3, 2, '', '/w', 'SH', NULL);
            "#,
        )
        .unwrap();
        // Store::init runs the CREATE IF NOT EXISTS + ALTER migration path.
        Store::init(&db).unwrap();
        let s = Store { db };
        let t = s.load_tree().unwrap();
        let p = &t.workspaces[0].tabs[0].panes[0];
        assert!(p.agent.is_none(), "legacy 'SH' reads as no agent");
        assert!(p.session_title.is_none());
        // Rows written before the size columns existed read back as the guess
        // they were already being given, rather than failing to load.
        assert_eq!((p.cols, p.rows), (80, 24));
        assert_eq!(s.load_agent_settings().unwrap(), crate::proto::AgentSettings::default());
    }

    #[test]
    fn a_pane_keeps_its_size_across_a_restart() {
        // The size used to be dropped on save and restored as a hardcoded
        // 80x24, so every pane respawned at a width the terminal did not have.
        let s = Store::in_memory().unwrap();
        let mut tree = SessionTree::default();
        let ws = tree.open_workspace("/w".into(), "w".into());
        let tab = tree.open_tab(ws).unwrap();
        {
            let p = &mut tree.workspaces[0].tabs[0].panes[0];
            p.cols = 203;
            p.rows = 55;
        }
        s.save_tree(&tree).unwrap();

        let back = s.load_tree().unwrap();
        let p = &back.workspaces[0].tabs[0].panes[0];
        assert_eq!((p.cols, p.rows), (203, 55), "the real viewport must survive a restart");
        let _ = tab;
    }
}
