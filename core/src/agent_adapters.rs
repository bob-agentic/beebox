//! Generates the on-disk agent adapter assets: the hook sender, the Claude
//! settings overlay, and the zsh shim that installs the `claude` wrapper
//! function without touching any user config file.
//!
//! Layout, under the daemon home (`~/.beebox`):
//!
//! ```text
//! hooks/
//!   send                  POSTs stdin to $BEEBOX_HOOK_URL; 500ms cap; exit 0
//!   claude-settings.json  hooks overlay passed via `claude --settings`
//!   claude-wrapper.zsh    the wrapper function
//!   zdot/.zshenv          ZDOTDIR shim, stage 1
//!   zdot/.zprofile        ZDOTDIR shim, login stage (Homebrew's PATH)
//!   zdot/.zshrc           ZDOTDIR shim, stage 2: user rc first, then wrapper
//!   zdot/.zlogin          ZDOTDIR shim, login stage 2 (after .zshrc)
//! ```
//!
//! One shim per startup file zsh reads, because panes run a login shell: a
//! missing one is a file the user wrote that silently never runs.
//!
//! Everything is regenerated at startup, so upgrades never leave stale
//! scripts behind. Nothing here writes outside the daemon home, and the shim
//! sources the user's own zsh files before adding anything — aliases, prompt,
//! PATH all behave exactly as in a plain terminal.
//!
//! bash and fish are later milestones; a pane whose shell is not zsh simply
//! gets no shim and `claude` runs unwrapped (fail open, agent unaffected).

use std::path::{Path, PathBuf};

use anyhow::Result;

pub struct AdapterAssets {
    /// `hooks/` — exported as BEEBOX_AGENT_HOOKS_DIR.
    pub hooks_dir: PathBuf,
    /// `hooks/zdot` — exported as ZDOTDIR for zsh panes.
    pub zdot_dir: PathBuf,
}

/// The hook sender. `sh`, not zsh: agents run hook commands through `sh -c`.
/// Guarantees the handover requires: bounded time, silent, and exit 0 no
/// matter what — a hook failure must never surface inside the agent.
///
/// With `send <agent> <event>` the payload is tagged so the daemon knows
/// which normalizer to use — Codex events arrive this way, mirroring how
/// mux0's agent-hook.sh passes the event in argv. Claude payloads carry their
/// own `hook_event_name` and come with no arguments.
///
/// Every event is stamped here, when it happened: each hook is its own curl,
/// they race to the daemon, and arrival order is not event order.
const SEND: &str = r#"#!/bin/sh
# BeeBox hook sender. Reads one JSON event on stdin and hands it to the
# daemon. Always exits 0: status is best-effort, the agent is not.
[ -n "$BEEBOX_HOOK_URL" ] || exit 0
payload=$(python3 -c '
import json, sys, time
try:
    body = json.load(sys.stdin)
except Exception:
    body = {}
if len(sys.argv) > 1:
    body["agent"] = sys.argv[1]
if len(sys.argv) > 2:
    body.setdefault("hook_event_name", sys.argv[2])
body["at_ms"] = int(time.time() * 1000)
print(json.dumps(body))
' "$@" 2>/dev/null) || exit 0
printf '%s' "$payload" | curl -s -o /dev/null --max-time 0.5 \
  -H 'Content-Type: application/json' --data-binary @- \
  "$BEEBOX_HOOK_URL" 2>/dev/null || true
exit 0
"#;

/// The wrapper function. Defined after the user's own rc, so their aliases
/// keep working and expand into this function by name.
const CLAUDE_WRAPPER: &str = r#"# BeeBox claude wrapper. Adds a hooks overlay via --settings; changes nothing
# else. Sourced by the ZDOTDIR shim after the user's own zshrc.
claude() {
  local real="${BEEBOX_CLAUDE_BIN:-}"
  if [ -z "$real" ]; then
    # `whence -p` resolves only executables on PATH, never this function —
    # `command -v` would report the function itself and loop.
    real="$(whence -p claude 2>/dev/null)"
  fi
  if [ -z "$real" ] || [ ! -x "$real" ]; then
    echo "claude: command not found" >&2
    return 127
  fi

  # Not a BeeBox pane (or hooks are broken): behave as if we did not exist.
  if [ -z "$BEEBOX_HOOK_URL" ] || [ -z "$BEEBOX_AGENT_HOOKS_DIR" ] \
     || [ ! -f "$BEEBOX_AGENT_HOOKS_DIR/claude-settings.json" ]; then
    "$real" "$@"
    return $?
  fi

  # One-shot subcommands are not agent turns; injecting a top-level
  # --settings into them would be wrong. `-p/--print` is NOT here: a print
  # run is a real turn even when its prompt text looks like a subcommand.
  case "$1" in
    mcp|doctor|auth|plugin|remote-control|install|update|migrate-installer|setup-token|--help|-h|--version|-v)
      "$real" "$@"
      return $?
      ;;
  esac

  "$real" --settings "$BEEBOX_AGENT_HOOKS_DIR/claude-settings.json" "$@"
}
"#;

/// Stage 1 of the ZDOTDIR shim. zsh reads `.zshenv` from $ZDOTDIR first; this
/// one plays the user's own `.zshenv` and then points zsh back at the shim so
/// stage 2 runs.
const ZSHENV: &str = r#"# BeeBox zsh shim (stage 1). Restores the user's real zsh startup files; the
# only addition happens at the end of .zshrc, stage 2.
_beebox_shim="$ZDOTDIR"
ZDOTDIR="${BEEBOX_USER_ZDOTDIR:-$HOME}"
[ -f "$ZDOTDIR/.zshenv" ] && . "$ZDOTDIR/.zshenv"
# Whatever the user's zshenv did, .zshrc must load from the shim exactly once.
ZDOTDIR="$_beebox_shim"
"#;

/// The login stage. A login shell reads `$ZDOTDIR/.zprofile`, and since
/// ZDOTDIR points at the shim, the user's own one would otherwise never run —
/// which on macOS means no Homebrew, because `brew shellenv` lives there.
const ZPROFILE: &str = r#"# BeeBox zsh shim (login stage). Plays the user's own .zprofile; a login
# shell is what makes PATH from Homebrew and friends reach every pane.
_beebox_shim="$ZDOTDIR"
ZDOTDIR="${BEEBOX_USER_ZDOTDIR:-$HOME}"
[ -f "$ZDOTDIR/.zprofile" ] && . "$ZDOTDIR/.zprofile"
ZDOTDIR="$_beebox_shim"
"#;

/// The last login stage. Runs *after* `.zshrc`, so skipping it would not just
/// drop the user's `.zlogin` — it would also let anything in there that the
/// wrapper depends on land in the wrong order.
///
/// `.zlogout` needs no shim: panes are killed, never exited cleanly, so it
/// would never run anyway.
const ZLOGIN: &str = r#"# BeeBox zsh shim (login stage 2), after .zshrc.
_beebox_shim="$ZDOTDIR"
ZDOTDIR="${BEEBOX_USER_ZDOTDIR:-$HOME}"
[ -f "$ZDOTDIR/.zlogin" ] && . "$ZDOTDIR/.zlogin"
ZDOTDIR="$_beebox_shim"
"#;

/// Stage 2. The user's rc runs first — with ZDOTDIR restored, so anything in
/// there that reads it sees the real value — then the wrapper lands on top.
const ZSHRC: &str = r#"# BeeBox zsh shim (stage 2).
_beebox_shim="$ZDOTDIR"
ZDOTDIR="${BEEBOX_USER_ZDOTDIR:-$HOME}"
[ -f "$ZDOTDIR/.zshrc" ] && . "$ZDOTDIR/.zshrc"
if [ -n "$BEEBOX_AGENT_HOOKS_DIR" ] && [ -f "$BEEBOX_AGENT_HOOKS_DIR/claude-wrapper.zsh" ]; then
  . "$BEEBOX_AGENT_HOOKS_DIR/claude-wrapper.zsh"
fi
if [ -n "$BEEBOX_AGENT_HOOKS_DIR" ] && [ -f "$BEEBOX_AGENT_HOOKS_DIR/codex-wrapper.zsh" ]; then
  . "$BEEBOX_AGENT_HOOKS_DIR/codex-wrapper.zsh"
fi
unset _beebox_shim
"#;

/// The codex wrapper *function*: finds the real binary and hands off to the
/// launcher script, which owns the overlay dance and cleanup traps. A
/// function cannot reliably run EXIT traps, a script can — mux0 learned the
/// same lesson (`exec` skips traps; see its codex-wrapper.sh).
const CODEX_WRAPPER: &str = r#"# BeeBox codex wrapper. Runs Codex against a stable overlay CODEX_HOME that
# adds BeeBox's hooks.json without touching ~/.codex/hooks.json.
codex() {
  local real="${BEEBOX_CODEX_BIN:-}"
  if [ -z "$real" ]; then
    real="$(whence -p codex 2>/dev/null)"
  fi
  if [ -z "$real" ] || [ ! -x "$real" ]; then
    echo "codex: command not found" >&2
    return 127
  fi
  if [ -z "$BEEBOX_HOOK_URL" ] || [ -z "$BEEBOX_AGENT_HOOKS_DIR" ] \
     || [ ! -x "$BEEBOX_AGENT_HOOKS_DIR/codex-run" ]; then
    "$real" "$@"
    return $?
  fi
  BEEBOX_REAL_CODEX="$real" "$BEEBOX_AGENT_HOOKS_DIR/codex-run" "$@"
}
"#;

/// The codex launcher. Structure follows mux0's codex-wrapper.sh, which this
/// implementation studied for behaviour (overlay lifecycle, rename(2)
/// write-back, trust stability) and re-implements independently:
///
/// - The overlay path is stable per user: Codex keys hook trust on the
///   absolute path of hooks.json, so a per-launch path would demand
///   re-approval every start.
/// - Before symlinking, any regular file in the overlay is copied back to the
///   real CODEX_HOME: Codex persists config via tempfile+rename(2), which
///   replaces our symlink with a real file — those writes (`codex login`,
///   `/hooks` trust) must not be lost, even after SIGKILL skipped the trap.
/// - Codex runs as a subprocess, not `exec`: shells do not fire EXIT traps
///   after a successful exec, and the trap is what syncs writes back.
/// - The overlay is never deleted: other panes' Codex processes share it.
const CODEX_RUN: &str = r#"#!/bin/zsh
# BeeBox codex launcher. See agent_adapters.rs for the full rationale.
set -e

real="${BEEBOX_REAL_CODEX:?}"
user_home="${BEEBOX_USER_CODEX_HOME:-${HOME}/.codex}"
# Beside hooks/, so a second daemon home keeps its own. `:h` rather than `..`:
# for ~/.beebox this must stay the exact path it always was, because Codex's
# hook trust keys on it.
overlay="${BEEBOX_AGENT_HOOKS_DIR:?}"
overlay="${overlay:h}/codex-overlay"
mkdir -p "$overlay"

sync_back() {
  # Anything that is now a regular file was written by Codex via rename(2);
  # copy it home. hooks.json is ours and stays ours.
  local item name
  for item in "$overlay"/*(N); do
    [ -f "$item" ] || continue
    [ -L "$item" ] && continue
    name="${item:t}"
    [ "$name" = "hooks.json" ] && continue
    mkdir -p "$user_home"
    cp -f "$item" "$user_home/$name" 2>/dev/null || true
  done
}

# Writes from a previous crash or a concurrently running pane, first.
sync_back

# Mirror the user's real CODEX_HOME into the overlay. `ln -sfn` atomically
# replaces whatever is there; safe because sync_back just ran.
if [ -d "$user_home" ]; then
  for item in "$user_home"/*(N); do
    name="${item:t}"
    [ "$name" = "hooks.json" ] && continue
    ln -sfn "$item" "$overlay/$name"
  done
fi
if [ ! -e "$overlay/config.toml" ] && [ ! -L "$overlay/config.toml" ]; then
  mkdir -p "$user_home"
  ln -sfn "$user_home/config.toml" "$overlay/config.toml"
fi

# Our hooks.json. Rewritten every launch so an upgraded send path lands, but
# the *content* is stable for a given install — Codex trust keys on
# path+command, and churn there would mean re-approving in /hooks each time.
send="$BEEBOX_AGENT_HOOKS_DIR/send"
cat > "$overlay/hooks.json" <<EOF
{
  "hooks": {
    "SessionStart":      [{"hooks": [{"type": "command", "command": "\"$send\" codex SessionStart", "timeout": 3}]}],
    "UserPromptSubmit":  [{"hooks": [{"type": "command", "command": "\"$send\" codex UserPromptSubmit", "timeout": 3}]}],
    "PreToolUse":        [{"hooks": [{"type": "command", "command": "\"$send\" codex PreToolUse", "timeout": 3}]}],
    "PostToolUse":       [{"hooks": [{"type": "command", "command": "\"$send\" codex PostToolUse", "timeout": 3}]}],
    "PermissionRequest": [{"hooks": [{"type": "command", "command": "\"$send\" codex PermissionRequest", "timeout": 3}]}],
    "Stop":              [{"hooks": [{"type": "command", "command": "\"$send\" codex Stop", "timeout": 3}]}]
  }
}
EOF

export CODEX_HOME="$overlay"
trap sync_back EXIT INT TERM

# Subprocess + wait, keeping the real exit code. `|| code=$?` stops set -e
# from skipping the trap-carrying exit path.
code=0
"$real" "$@" || code=$?
exit "$code"
"#;

/// The hooks overlay handed to `claude --settings`. Commands resolve the
/// sender through the environment at fire time, so the file itself is static
/// and carries no per-pane secret.
fn claude_settings() -> String {
    let send = r#""$BEEBOX_AGENT_HOOKS_DIR/send""#;
    let one = |_event: &str| {
        format!(
            r#"[{{"hooks":[{{"type":"command","command":{},"timeout":3}}]}}]"#,
            serde_json::to_string(send).unwrap()
        )
    };
    format!(
        r#"{{
  "hooks": {{
    "SessionStart": {ss},
    "UserPromptSubmit": {ups},
    "PreToolUse": {pre},
    "PostToolUse": {post},
    "Notification": {n},
    "Stop": {stop},
    "SessionEnd": {se}
  }}
}}
"#,
        ss = one("SessionStart"),
        ups = one("UserPromptSubmit"),
        pre = one("PreToolUse"),
        post = one("PostToolUse"),
        n = one("Notification"),
        stop = one("Stop"),
        se = one("SessionEnd"),
    )
}

/// Writes (or rewrites) every adapter asset under `home`. Idempotent and
/// cheap; called once per daemon start.
pub fn install(home: &Path) -> Result<AdapterAssets> {
    use std::os::unix::fs::PermissionsExt;

    let hooks_dir = home.join("hooks");
    let zdot_dir = hooks_dir.join("zdot");
    std::fs::create_dir_all(&zdot_dir)?;

    let send = hooks_dir.join("send");
    std::fs::write(&send, SEND)?;
    std::fs::set_permissions(&send, std::fs::Permissions::from_mode(0o755))?;

    std::fs::write(hooks_dir.join("claude-settings.json"), claude_settings())?;
    std::fs::write(hooks_dir.join("claude-wrapper.zsh"), CLAUDE_WRAPPER)?;
    std::fs::write(hooks_dir.join("codex-wrapper.zsh"), CODEX_WRAPPER)?;
    let codex_run = hooks_dir.join("codex-run");
    std::fs::write(&codex_run, CODEX_RUN)?;
    std::fs::set_permissions(&codex_run, std::fs::Permissions::from_mode(0o755))?;
    // All four, because panes run a login shell: zsh then reads .zshenv,
    // .zprofile, .zshrc and .zlogin from ZDOTDIR, and any file missing here is
    // one the user wrote and never sees run.
    std::fs::write(zdot_dir.join(".zshenv"), ZSHENV)?;
    std::fs::write(zdot_dir.join(".zprofile"), ZPROFILE)?;
    std::fs::write(zdot_dir.join(".zshrc"), ZSHRC)?;
    std::fs::write(zdot_dir.join(".zlogin"), ZLOGIN)?;

    Ok(AdapterAssets { hooks_dir, zdot_dir })
}

/// Environment for one pane's PTY. Only zsh gets the shim; other shells run
/// exactly as before (bash/fish are later milestones, and fail open is the
/// rule).
pub fn pane_env(assets: &AdapterAssets, shell: &str) -> Vec<(String, String)> {
    let mut env = vec![(
        "BEEBOX_AGENT_HOOKS_DIR".to_string(),
        assets.hooks_dir.to_string_lossy().into_owned(),
    )];
    if shell.rsplit('/').next() == Some("zsh") {
        // The user's own ZDOTDIR (usually unset) has to survive the shim.
        if let Ok(user) = std::env::var("ZDOTDIR") {
            env.push(("BEEBOX_USER_ZDOTDIR".to_string(), user));
        }
        env.push((
            "ZDOTDIR".to_string(),
            assets.zdot_dir.to_string_lossy().into_owned(),
        ));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch dir that cleans up after itself, without a crate for it.
    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tmp() -> Tmp {
        let p = std::env::temp_dir().join(format!(
            "beebox-adapters-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }

    #[test]
    fn install_writes_everything_and_is_idempotent() {
        let dir = tmp();
        let a = install(&dir.0).unwrap();
        let b = install(&dir.0).unwrap();
        assert_eq!(a.hooks_dir, b.hooks_dir);
        for f in ["send", "claude-settings.json", "claude-wrapper.zsh"] {
            assert!(a.hooks_dir.join(f).is_file(), "{f} missing");
        }
        assert!(a.zdot_dir.join(".zshenv").is_file());
        assert!(a.zdot_dir.join(".zprofile").is_file());
        assert!(a.zdot_dir.join(".zshrc").is_file());
        assert!(a.zdot_dir.join(".zlogin").is_file());

        // The settings must be valid JSON with all seven events wired.
        let s: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(a.hooks_dir.join("claude-settings.json")).unwrap(),
        )
        .unwrap();
        let hooks = s.get("hooks").unwrap().as_object().unwrap();
        for ev in [
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "Notification",
            "Stop",
            "SessionEnd",
        ] {
            assert!(hooks.contains_key(ev), "{ev} not hooked");
        }
    }

    #[test]
    fn the_shim_forwards_every_startup_file_zsh_reads() {
        // Panes run `zsh -l -i`, so zsh looks for all four of these inside
        // ZDOTDIR — which is the shim. Any one missing is a file the user
        // wrote that silently stops running, and for .zprofile that means no
        // Homebrew. This pins the pairing: add a startup file to the shell's
        // argv and you must add its shim here.
        let dir = tmp();
        let a = install(&dir.0).unwrap();

        for name in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
            let body = std::fs::read_to_string(a.zdot_dir.join(name)).unwrap();
            assert!(
                body.contains(&format!("$ZDOTDIR/{name}")),
                "{name} shim must source the user's own {name}"
            );
            assert!(
                body.contains("BEEBOX_USER_ZDOTDIR"),
                "{name} shim must honour a user-set ZDOTDIR"
            );
        }
    }

    #[test]
    fn only_zsh_panes_get_the_zdotdir_shim() {
        let dir = tmp();
        let a = install(&dir.0).unwrap();

        let zsh: std::collections::HashMap<_, _> =
            pane_env(&a, "/bin/zsh").into_iter().collect();
        assert!(zsh.contains_key("ZDOTDIR"));
        assert!(zsh.contains_key("BEEBOX_AGENT_HOOKS_DIR"));

        let bash: std::collections::HashMap<_, _> =
            pane_env(&a, "/bin/bash").into_iter().collect();
        assert!(!bash.contains_key("ZDOTDIR"), "bash must be untouched for now");
        assert!(bash.contains_key("BEEBOX_AGENT_HOOKS_DIR"));
    }
}
