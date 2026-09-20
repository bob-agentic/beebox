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

/// Runs forever. One task for the whole app, not one per pane: a sweep every
/// two seconds over a handful of pids is nothing, and there is exactly one
/// place to reason about the cost.
pub async fn poll_forever(app: Arc<App>) {
    struct PaneState {
        cwd: String,
        git_at: std::time::Instant,
    }
    let mut states: HashMap<u64, PaneState> = HashMap::new();
    let mut sys = sysinfo::System::new();

    loop {
        tokio::time::sleep(CWD_EVERY).await;

        let live = app.ptys.live_pids();
        states.retain(|pane, _| live.iter().any(|(p, _)| p == pane));

        for (pane, pid) in live {
            // The syscall is blocking but sub-millisecond; git is the part
            // that must not run per tick.
            let Some(cwd) = cwd_of(&mut sys, pid) else { continue };

            let (cwd_changed, git_due) = match states.get(&pane) {
                Some(s) => (s.cwd != cwd, s.git_at.elapsed() >= GIT_EVERY),
                None => (true, true),
            };
            if !cwd_changed && !git_due {
                continue;
            }

            // Git in a blocking task; the poller must not stall the runtime.
            let git = {
                let cwd = cwd.clone();
                tokio::task::spawn_blocking(move || git_info(&cwd))
                    .await
                    .unwrap_or(None)
            };
            states.insert(
                pane,
                PaneState { cwd: cwd.clone(), git_at: std::time::Instant::now() },
            );
            app.update_cwd(pane, cwd, git).await;
        }
    }
}
