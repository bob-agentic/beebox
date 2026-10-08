//! cwd + git polling. ARCHITECTURE.md §7: the pane footer follows the shell's
//! real working directory via `proc_pidinfo`, never OSC 7 — the user's shell
//! config stays untouched. Third-party `darwin-libproc` does the syscall.
//!
//! Git status is `git status --porcelain=v2 --branch`, run only when the cwd
//! changed plus a slow refresh; fifteen busy panes polling git unthrottled
//! would spawn subprocesses forever, so the debounce is load-bearing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::app::App;
use crate::proto::GitInfo;
use crate::session::WsGit;

/// How often the foreground cwd is sampled. Cheap (one syscall per pane).
const CWD_EVERY: Duration = Duration::from_secs(2);
/// Git refresh floor for an unchanged cwd. Branch/dirty state moves slowly.
const GIT_EVERY: Duration = Duration::from_secs(15);

/// The shell's current working directory, via `sysinfo` (mature third-party;
/// wraps proc_pidinfo on macOS, /proc on Linux). One System reused across
/// sweeps — constructing it fresh each tick would rescan everything.
fn cwd_of(sys: &mut sysinfo::System, pid: u32) -> Option<String> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, UpdateKind};
    let target = Pid::from_u32(pid);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[target]),
        true,
        ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always),
    );
    sys.process(target)
        .and_then(|p| p.cwd())
        .map(|p| p.to_string_lossy().into_owned())
}

fn git_info(cwd: &str) -> Option<GitInfo> {
    let out = std::process::Command::new("git")
        .args(["status", "--porcelain=v2", "--branch"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut branch = String::new();
    let mut added = 0u32;
    let mut modified = 0u32;
    for line in text.lines() {
        if let Some(b) = line.strip_prefix("# branch.head ") {
            branch = b.to_string();
        } else if line.starts_with('?') {
            added += 1;
        } else if line.starts_with('1') || line.starts_with('2') || line.starts_with('u') {
            modified += 1;
        }
    }
    (!branch.is_empty()).then_some(GitInfo { branch, added, modified })
}

/// Where a workspace folder sits in git. One `rev-parse` answers all three
/// questions; `--path-format=absolute` (git 2.31) so the common dir compares
/// equal from every worktree of the repository.
pub fn ws_git(path: &str) -> Option<WsGit> {
    let out = std::process::Command::new("git")
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
            "--abbrev-ref",
            "HEAD",
        ])
        .current_dir(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let git_dir = lines.next()?.to_string();
    let common_dir = lines.next()?.to_string();
    let branch = lines.next().unwrap_or_default().to_string();
    Some(WsGit { linked: git_dir != common_dir, branch, common_dir })
}

/// Uncommitted files in a worktree, untracked ones included — what deleting
/// the folder would throw away.
pub fn dirty_count(path: &str) -> u32 {
    std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().count() as u32)
        .unwrap_or(0)
}

/// Looks at every workspace folder: git first, straight away, so worktrees
/// are grouped from the first frame, and then on the slow git cadence. One
/// pass per tick for all of them; a handful of `rev-parse`s is nothing.
pub async fn poll_workspaces(app: Arc<App>) {
    let mut looking = app.attention();
    loop {
        // Nobody looking: wait for someone, then refresh at once rather than
        // showing them whatever was true when they left.
        attended(&mut looking).await;
        let folders: Vec<(u64, String)> = {
            let tree = app.tree.lock().await;
            tree.workspaces.iter().map(|w| (w.id, w.path.clone())).collect()
        };
        // One blocking task for the whole sweep.
        let sweep = tokio::task::spawn_blocking(move || {
            folders
                .into_iter()
                .map(|(ws, path)| (ws, ws_git(&path), std::path::Path::new(&path).is_dir()))
                .collect()
        })
        .await
        .unwrap_or_default();
        app.apply_ws_git(sweep).await;
        tokio::time::sleep(GIT_EVERY).await;
    }
}

/// Returns once some window has focus.
async fn attended(rx: &mut tokio::sync::watch::Receiver<bool>) {
    while !*rx.borrow_and_update() {
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Runs forever. One task for the whole app, not one per pane: a sweep every
/// two seconds over a handful of pids is nothing, and there is exactly one
/// place to reason about the cost.
pub async fn poll_forever(app: Arc<App>) {
    /// What each pane was last told.
    struct PaneState {
        cwd: String,
        git: Option<GitInfo>,
    }
    let mut states: HashMap<u64, PaneState> = HashMap::new();
    // Git is asked per folder, not per pane: a dozen panes in one repository
    // share one `git status`.
    let mut by_dir: HashMap<String, (std::time::Instant, Option<GitInfo>)> = HashMap::new();
    let mut sys = sysinfo::System::new();
    let looking = app.attention();

    loop {
        tokio::time::sleep(CWD_EVERY).await;

        let live = app.ptys.live_pids();
        let alive: std::collections::HashSet<u64> = live.iter().map(|(p, _)| *p).collect();
        states.retain(|pane, _| alive.contains(pane));
        // The cwd is sampled regardless — it is persisted, for resuming in the
        // right place. The slow git refresh waits for a window with focus; a
        // folder git has not been asked about yet is asked at once, so the
        // footer never shows the old repository's branch.
        let attended = *looking.borrow();

        for (pane, pid) in live {
            // The syscall is blocking but sub-millisecond; git is the part
            // that must not run per tick.
            let Some(cwd) = cwd_of(&mut sys, pid) else { continue };

            let due = match by_dir.get(&cwd) {
                Some((at, _)) => attended && at.elapsed() >= GIT_EVERY,
                None => true,
            };
            if due {
                // In a blocking task; the poller must not stall the runtime.
                let dir = cwd.clone();
                let git = tokio::task::spawn_blocking(move || git_info(&dir)).await.unwrap_or(None);
                by_dir.insert(cwd.clone(), (std::time::Instant::now(), git));
            }
            let git = by_dir.get(&cwd).and_then(|(_, g)| g.clone());

            let fresh = states.get(&pane).is_none_or(|s| s.cwd != cwd || s.git != git);
            if fresh {
                states.insert(pane, PaneState { cwd: cwd.clone(), git: git.clone() });
                app.update_cwd(pane, cwd, git).await;
            }
        }
        // Folders no pane is in any more.
        let dirs: std::collections::HashSet<&String> = states.values().map(|s| &s.cwd).collect();
        by_dir.retain(|d, _| dirs.contains(d));
    }
}
