//! The Claude vertical slice, end to end inside Rust: a real zsh in a real
//! PTY, the ZDOTDIR shim, the wrapper function, a fake `claude` binary, the
//! `send` script, the HTTP hook endpoint, and the state machine — everything
//! except a browser.
//!
//! The fake `claude` behaves the way the real one does at the seam we care
//! about: it receives `--settings <file>`, resolves the hook command from
//! that file, and pipes each lifecycle event into it. If the wrapper or the
//! sender misbehaves, this test — not a paying user — finds out.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use beebox_core::app::App;
use beebox_core::proto::AgentPhase;
use beebox_core::pty::PtyEvent;
use beebox_core::store::Store;

struct Tmp(std::path::PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn tmp(name: &str) -> Tmp {
    let p = std::env::temp_dir().join(format!(
        "beebox-slice-{name}-{}-{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    Tmp(p)
}

/// A fake `claude`. Asserts it was given the BeeBox settings overlay, then
/// fires the same hook sequence the real CLI would for one prompt with one
/// failing tool, using the hook command from the settings file itself.
fn write_fake_claude(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("claude");
    let script = r#"#!/bin/zsh
# Fake Claude Code for the vertical-slice test.
settings=""
if [ "$1" = "--settings" ]; then settings="$2"; fi
if [ -z "$settings" ]; then
  echo "FAKE_CLAUDE_NO_SETTINGS"
  exit 3
fi
send="$BEEBOX_AGENT_HOOKS_DIR/send"
fire() { printf '%s' "$1" | "$send"; }
fire '{"hook_event_name":"SessionStart","session_id":"fake-sess-1","at_ms":1000}'
fire '{"hook_event_name":"UserPromptSubmit","session_id":"fake-sess-1","prompt":"do the thing","at_ms":2000}'
fire '{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"make test"},"at_ms":3000}'
fire '{"hook_event_name":"PostToolUse","tool_response":{"is_error":true},"at_ms":4000}'
fire '{"hook_event_name":"Stop","at_ms":5000}'
echo "FAKE_CLAUDE_DONE"
exit 0
"#;
    std::fs::write(&path, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

async fn served(home: &Path) -> (Arc<App>, u16) {
    let app = App::new_with_adapters(Store::in_memory().unwrap(), 1000, home);
    app.bootstrap().await.unwrap();
    // bootstrap no longer opens a folder for us; these tests need one pane.
    {
        let mut t = app.tree.lock().await;
        let ws = t.open_workspace("/tmp".into(), "tmp".into());
        t.open_tab(ws).unwrap();
    }
    // Toggles default OFF; this slice tests the enabled path.
    app.set_agent_setting(
        beebox_core::proto::AgentKind::Claude,
        beebox_core::proto::AgentSetting::Status,
        true,
    )
    .await;
    app.set_agent_setting(
        beebox_core::proto::AgentKind::Claude,
        beebox_core::proto::AgentSetting::Resume,
        true,
    )
    .await;
    let router = beebox_core::http::router(app.clone(), None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    app.set_hook_port(port);
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (app, port)
}

/// Types into a PTY and collects output until `marker` appears or `secs`
/// elapse. Panics with everything seen so far when it never shows up.
async fn wait_for(
    rx: &mut tokio::sync::broadcast::Receiver<PtyEvent>,
    marker: &str,
    secs: u64,
) -> String {
    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while !seen.contains(marker) {
        let left = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or_else(|| panic!("never saw {marker:?}; output so far:\n{seen}"));
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Ok(PtyEvent::Output { data, .. })) => {
                seen.push_str(&String::from_utf8_lossy(&data));
            }
            Ok(Ok(_)) => {}
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(_)) | Err(_) => panic!("pty gone; output so far:\n{seen}"),
        }
    }
    seen
}

/// Env mutations (`ZDOTDIR`) are process-wide; the slice tests take this so
/// they cannot race each other's spawns.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One pane, spawned exactly the way the app spawns them (through
/// ensure_running), running a real interactive zsh through the shim.
async fn spawn_shell_pane(app: &Arc<App>) -> (u64, u64) {
    let pane = app.tree.lock().await.workspaces[0].tabs[0].panes[0].id;
    // ensure_running already ran at bootstrap without the hook port; respawn
    // so the env (hook URL + shim) lands. This is the same path a respawned
    // pane takes in production.
    app.mark_exited(pane).await;
    let pty = app.ensure_running(pane).await.unwrap();
    (pane, pty)
}

/// Types the override + command. The user's own zshrc may rewrite PATH (p10k
/// setups do), so tests resolve the fake binary through the wrapper's
/// explicit `BEEBOX_CLAUDE_BIN` override — which exists for exactly this.
fn claude_cmd(bin: &Path, rest: &str) -> Vec<u8> {
    format!(
        "export BEEBOX_CLAUDE_BIN={}/claude; {rest}\r",
        bin.display()
    )
    .into_bytes()
}

#[tokio::test(flavor = "multi_thread")]
async fn typing_claude_in_a_real_pty_drives_the_status_machine() {
    if std::path::Path::new("/bin/zsh").exists() {
        let home = tmp("drive");
        let bin = tmp("bin");
        write_fake_claude(&bin.0);

        let (app, _port) = served(&home.0).await;
        let mut rx = app.ptys.subscribe();
        let (pane, pty) = {
            let _g = ENV_LOCK.lock().unwrap();
            spawn_shell_pane(&app).await
        };

        // A real zsh prompt through the shim proves the user's startup files
        // did not break; then the user types `claude`, like any day. Resolved
        // via PATH, not the test override — the smoke test caught a wrapper
        // that reported its own function as the "binary" and looped.
        tokio::time::sleep(Duration::from_millis(800)).await;
        app.ptys
            .write(
                pty,
                format!("export PATH={}:$PATH; claude\r", bin.0.display()).as_bytes(),
            )
            .unwrap();
        let out = wait_for(&mut rx, "FAKE_CLAUDE_DONE", 15).await;
        assert!(
            !out.contains("FAKE_CLAUDE_NO_SETTINGS"),
            "wrapper failed to inject --settings:\n{out}"
        );

        // Hooks land asynchronously; poll briefly for the final state.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let view = app
                .tree
                .lock()
                .await
                .pane(pane)
                .unwrap()
                .status
                .view()
                .clone();
            if view.phase == AgentPhase::Failed {
                assert_eq!(view.agent, Some(beebox_core::proto::AgentKind::Claude));
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "hooks never drove the pane to failed; stuck at {:?}",
                view.phase
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // And the session id was captured for resume.
        assert_eq!(
            app.tree.lock().await.pane(pane).unwrap().session_ref.as_deref(),
            Some("fake-sess-1")
        );
    } else {
        eprintln!("no /bin/zsh; skipping");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_still_works_when_the_hook_url_is_dead() {
    if std::path::Path::new("/bin/zsh").exists() {
        let home = tmp("dead");
        let bin = tmp("bin2");
        write_fake_claude(&bin.0);

        let (app, _port) = served(&home.0).await;
        let mut rx = app.ptys.subscribe();
        let (pane, pty) = {
            let _g = ENV_LOCK.lock().unwrap();
            spawn_shell_pane(&app).await
        };

        // Point the sender at a port nothing listens on. The agent must not
        // notice: `send` swallows the failure and exits 0.
        tokio::time::sleep(Duration::from_millis(800)).await;
        app.ptys
            .write(pty, b"export BEEBOX_HOOK_URL=http://127.0.0.1:1/hooks/1/x\r")
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        // `HOOKLESS_` + `OK` in the typed command, so the echo of the command
        // itself cannot satisfy the wait below — only the executed output can.
        app.ptys
            .write(pty, &claude_cmd(&bin.0, "claude && echo HOOKLESS_''OK"))
            .unwrap();

        let out = wait_for(&mut rx, "HOOKLESS_OK", 15).await;
        assert!(out.contains("FAKE_CLAUDE_DONE"), "agent broke without hooks:\n{out}");
        // No hook arrived, so the pane never left its initial state.
        assert_eq!(
            app.tree.lock().await.pane(pane).unwrap().status.view().phase,
            AgentPhase::NeverRan
        );
    } else {
        eprintln!("no /bin/zsh; skipping");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn user_aliases_survive_the_shim() {
    if std::path::Path::new("/bin/zsh").exists() {
        let home = tmp("alias");
        let bin = tmp("bin3");
        write_fake_claude(&bin.0);

        // A user zshrc with an alias that must keep expanding to the wrapper.
        let user_home = tmp("userhome");
        std::fs::write(
            user_home.0.join(".zshrc"),
            "alias cl='claude'\nexport BEEBOX_RC_RAN=1\n",
        )
        .unwrap();

        let (app, _port) = served(&home.0).await;
        let mut rx = app.ptys.subscribe();

        let pane = app.tree.lock().await.workspaces[0].tabs[0].panes[0].id;
        app.mark_exited(pane).await;
        // Redirect the shim's idea of "the user's zsh files" at our fixture.
        // In production BEEBOX_USER_ZDOTDIR defaults to $HOME. Guarded: env
        // is process-global and another slice test may be spawning.
        let pty = {
            let _g = ENV_LOCK.lock().unwrap();
            std::env::set_var("ZDOTDIR", &user_home.0);
            let pty = app.ensure_running(pane).await.unwrap();
            std::env::remove_var("ZDOTDIR");
            pty
        };

        tokio::time::sleep(Duration::from_millis(800)).await;
        // The alias from the user's own rc must reach the wrapper function.
        app.ptys
            .write(pty, &claude_cmd(&bin.0, "echo rc=$BEEBOX_RC_RAN && cl"))
            .unwrap();
        let out = wait_for(&mut rx, "FAKE_CLAUDE_DONE", 15).await;
        assert!(out.contains("rc=1"), "user zshrc did not run:\n{out}");
        assert!(!out.contains("FAKE_CLAUDE_NO_SETTINGS"), "alias bypassed the wrapper:\n{out}");
    } else {
        eprintln!("no /bin/zsh; skipping");
    }
}
