//! The Codex vertical slice: the zsh wrapper function, the codex-run
//! launcher, the stable overlay CODEX_HOME, rename(2) write-back, hook
//! delivery, and exit-code preservation — with a fake `codex` binary that
//! does what the real one does at this seam: reads CODEX_HOME, fires the
//! hooks from hooks.json, and rewrites config.toml via tempfile+rename.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use beebox_core::app::App;
use beebox_core::proto::{AgentKind, AgentPhase, AgentSetting};
use beebox_core::pty::PtyEvent;
use beebox_core::store::Store;

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn tmp(name: &str) -> Tmp {
    let p = std::env::temp_dir().join(format!(
        "beebox-cx-{name}-{}-{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    Tmp(p)
}

/// A fake `codex`. Verifies it was pointed at an overlay CODEX_HOME with a
/// BeeBox hooks.json, fires one prompt→pretool→posttool→stop sequence through
/// the hook commands in that file, rewrites config.toml the way the real
/// binary does (tempfile + rename, which replaces the symlink), then exits 7.
fn write_fake_codex(dir: &Path) -> PathBuf {
    let path = dir.join("codex");
    let script = r#"#!/bin/zsh
[ -n "$CODEX_HOME" ] || { echo FAKE_CODEX_NO_HOME; exit 3; }
[ -f "$CODEX_HOME/hooks.json" ] || { echo FAKE_CODEX_NO_HOOKS; exit 3; }
# Fire the hook command for each event, as real codex does. The command
# string is `"<send>" codex <Event>`; extract and run it via sh -c with a
# JSON payload on stdin.
fire() {
  local event="$1" payload="$2"
  local cmd
  cmd=$(python3 -c '
import json,sys
h=json.load(open(sys.argv[1]))
print(h["hooks"][sys.argv[2]][0]["hooks"][0]["command"])
' "$CODEX_HOME/hooks.json" "$event")
  printf '%s' "$payload" | sh -c "$cmd"
}
fire UserPromptSubmit '{"session_id":"cx-slice-1","prompt":"do codex things"}'
fire PreToolUse '{"tool_name":"Bash","tool_input":{"command":"make"}}'
fire PostToolUse '{"tool_response":{"is_error":false}}'
fire Stop '{"last_assistant_message":"Codex finished the task."}'
# Rewrite config.toml the way codex persists: tempfile + rename, replacing
# whatever directory entry (symlink) is there.
echo 'written_by = "fake-codex"' > "$CODEX_HOME/.config.tmp"
mv -f "$CODEX_HOME/.config.tmp" "$CODEX_HOME/config.toml"
echo FAKE_CODEX_DONE
exit 7
"#;
    std::fs::write(&path, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

async fn served(home: &Path) -> (Arc<App>, u16) {
    let app = App::new_with_adapters(Store::in_memory().unwrap(), 1000, home);
    app.bootstrap("/tmp".into()).await.unwrap();
    app.set_agent_setting(AgentKind::Codex, AgentSetting::Status, true).await;
    app.set_agent_setting(AgentKind::Codex, AgentSetting::Resume, true).await;
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

#[tokio::test(flavor = "multi_thread")]
async fn typing_codex_drives_status_and_syncs_config_back() {
    if !Path::new("/bin/zsh").exists() {
        eprintln!("no /bin/zsh; skipping");
        return;
    }
    let daemon_home = tmp("home");
    let bin = tmp("bin");
    write_fake_codex(&bin.0);

    // A fake user CODEX_HOME with a pre-existing config the overlay must
    // mirror, and a hooks.json of the user's own that must never be touched.
    let user_codex = tmp("usercodex");
    std::fs::write(user_codex.0.join("config.toml"), "original = true\n").unwrap();
    std::fs::write(user_codex.0.join("hooks.json"), "{\"user\":\"own\"}").unwrap();

    let (app, _port) = served(&daemon_home.0).await;
    let mut rx = app.ptys.subscribe();

    let pane = app.tree.lock().await.workspaces[0].tabs[0].panes[0].id;
    app.mark_exited(pane).await;
    let pty = app.ensure_running(pane).await.unwrap();

    tokio::time::sleep(Duration::from_millis(800)).await;
    // BEEBOX_USER_CODEX_HOME points the launcher at the fixture instead of
    // the real ~/.codex; the wrapper resolves the fake binary explicitly.
    // Overlay path also redirected under the fixture so tests never touch
    // the real ~/.beebox.
    app.ptys
        .write(
            pty,
            format!(
                "export BEEBOX_CODEX_BIN={}/codex BEEBOX_USER_CODEX_HOME={} HOME={}; codex --full-auto; echo EXIT_''CODE=$?\r",
                bin.0.display(),
                user_codex.0.display(),
                daemon_home.0.display(),
            )
            .as_bytes(),
        )
        .unwrap();

    let out = wait_for(&mut rx, "EXIT_CODE=", 15).await;
    assert!(out.contains("FAKE_CODEX_DONE"), "codex never ran:\n{out}");
    assert!(!out.contains("FAKE_CODEX_NO_HOOKS"), "overlay hooks.json missing:\n{out}");
    assert!(out.contains("EXIT_CODE=7"), "real exit code must be preserved:\n{out}");

    // Hooks drove the pane: prompt → … → stop with no error = success.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let v = app.tree.lock().await.pane(pane).unwrap().status.view().clone();
        if v.phase == AgentPhase::Success {
            assert_eq!(v.agent, Some(AgentKind::Codex));
            assert_eq!(v.summary.as_deref(), Some("Codex finished the task."));
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "codex hooks never drove the pane; stuck at {:?}",
            v.phase
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        app.tree.lock().await.pane(pane).unwrap().session_ref.as_deref(),
        Some("cx-slice-1")
    );

    // config.toml written by codex (rename over the symlink) was synced back
    // to the user's real CODEX_HOME by the EXIT trap.
    let synced = std::fs::read_to_string(user_codex.0.join("config.toml")).unwrap();
    assert!(synced.contains("fake-codex"), "write-back lost: {synced}");
    // And the user's own hooks.json was never overwritten.
    let user_hooks = std::fs::read_to_string(user_codex.0.join("hooks.json")).unwrap();
    assert_eq!(user_hooks, "{\"user\":\"own\"}");

    // The overlay is stable and survives the exit (trust depends on it).
    let overlay = daemon_home.0.join(".beebox/codex-overlay");
    assert!(overlay.join("hooks.json").is_file(), "overlay must not be deleted");
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_passthrough_when_beebox_env_is_missing() {
    if !Path::new("/bin/zsh").exists() {
        eprintln!("no /bin/zsh; skipping");
        return;
    }
    let daemon_home = tmp("home2");
    let bin = tmp("bin2");
    // A fake codex that just proves it ran without an overlay.
    let path = bin.0.join("codex");
    std::fs::write(
        &path,
        "#!/bin/zsh\nif [ -n \"$CODEX_HOME\" ]; then echo HAS_OVERLAY; else echo NO_OVERLAY; fi\nexit 0\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (app, _port) = served(&daemon_home.0).await;
    let mut rx = app.ptys.subscribe();
    let pane = app.tree.lock().await.workspaces[0].tabs[0].panes[0].id;
    app.mark_exited(pane).await;
    let pty = app.ensure_running(pane).await.unwrap();

    tokio::time::sleep(Duration::from_millis(800)).await;
    // Clearing BEEBOX_HOOK_URL simulates a pane outside BeeBox: the wrapper
    // must hand straight through to the real binary, overlay untouched.
    app.ptys
        .write(
            pty,
            format!(
                "unset BEEBOX_HOOK_URL; export BEEBOX_CODEX_BIN={}/codex; codex; echo PASS_''OK\r",
                bin.0.display()
            )
            .as_bytes(),
        )
        .unwrap();
    let out = wait_for(&mut rx, "PASS_OK", 15).await;
    assert!(out.contains("NO_OVERLAY"), "passthrough must not set CODEX_HOME:\n{out}");
}
