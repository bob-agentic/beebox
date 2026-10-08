//! Generates the on-disk agent adapter assets: the hook sender, the Claude
//! settings overlay, and the zsh shim that installs the `claude` wrapper
//! function without touching any user config file.
//!
//! Layout, under the daemon home (`~/.beebox`):
//!
//! ```text
//! hooks/
//!   send                  POSTs stdin to $BEEBOX_HOOK_URL; 500ms cap; exit 0
//!   bin/beebox            the `beebox` command (workspaces and tabs)
//!   claude-settings.json  hooks overlay passed via `claude --settings`
//!   claude-plugin/        the `beebox` skill, via `claude --plugin-dir`
//!   codex-context.json    what Codex's SessionStart hook tells it
//!   codex-hooks           merges BeeBox's Codex hooks with the user's own
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
# Codex reads a SessionStart hook's stdout as context: how it learns that
# `beebox` exists, without its hook command — which trust is keyed on — changing.
if [ "$1" = codex ] && [ "$2" = SessionStart ] && [ -f "$BEEBOX_AGENT_HOOKS_DIR/codex-context.json" ]; then
  cat "$BEEBOX_AGENT_HOOKS_DIR/codex-context.json"
fi
exit 0
"#;

/// The `beebox` command. On PATH only inside a BeeBox zsh pane (the shim puts
/// `hooks/bin` there), and useless outside one anyway: it speaks for the pane
/// it runs in, with that pane's credentials from the environment.
///
/// Structure only — open, list, close. It never reads or types into another
/// terminal; agents talk to each other through their own channels.
const BEEBOX_CLI: &str = r#"#!/usr/bin/env python3
"""beebox — open BeeBox workspaces and tabs from inside a BeeBox terminal.

  beebox check                     may this terminal use the rest? (exit 0/1)
  beebox workspace open PATH [--name N] [-- CMD...]
  beebox tab new [--ws ID] [--name N] [--cwd DIR] [-- CMD...]
                                   (default ws: this terminal's)
  beebox workspace close ID        ends its processes; deletes nothing
  beebox list [--json]             workspaces/tabs/panes, agent session ids;
                                   "self" is this terminal

Output is one JSON line. CMD is typed at the new terminal's first prompt
(aliases work, the shell stays after). New tabs/workspaces open in the
background. A git worktree shows under its repository; `git worktree remove`
closes its workspace.

Delegating:
  1. beebox check. Refused: stop, tell the user, run nothing (no git).
  2. Default: beebox tab new --name N -- claude --name N "BRIEF"
     (Codex: -- codex "BRIEF").
  3. Worktree only if the user asks, only in a git repo (never git init):
     git worktree add MAIN.worktrees/BRANCH -b BRANCH
     (MAIN = first line of `git worktree list`), copy .env etc., then
     beebox workspace open THAT_PATH -- claude --name BRANCH "BRIEF".
     It branches from HEAD; uncommitted work stays behind — say so.
  4. BRIEF: goal, acceptance criteria, key files, and "questions or done:
     message <you>, conclusions only". Later: Claude SendMessage <name>;
     Codex `codex queue --thread <session> --message ...`.
  5. Merging, removing worktrees, deleting branches: ask the user.
"""
import json, os, shlex, sys, urllib.error, urllib.request

def die(msg, code=1):
    print(f"beebox: {msg}", file=sys.stderr)
    sys.exit(code)

def call(body):
    url = os.environ.get("BEEBOX_CLI_URL")
    if not url:
        die("not inside a BeeBox terminal")
    req = urllib.request.Request(url, data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=15) as r:
            return json.load(r)
    except urllib.error.HTTPError as e:
        try:
            msg = json.load(e).get("error") or f"HTTP {e.code}"
        except Exception:
            msg = f"HTTP {e.code}"
        if e.code == 403 and "stale" in msg:
            msg = "BeeBox is not reachable (restart this terminal if BeeBox was restarted)"
        die(msg)
    except (urllib.error.URLError, OSError):
        die("BeeBox is not reachable (restart this terminal if BeeBox was restarted)")

def split_cmd(argv):
    if "--" in argv:
        i = argv.index("--")
        return argv[:i], (shlex.join(argv[i + 1:]) or None)
    return argv, None

def take(args, flag):
    if flag in args:
        i = args.index(flag)
        if i + 1 >= len(args):
            die(f"{flag} needs a value", 2)
        v = args[i + 1]
        del args[i:i + 2]
        return v
    return None

def absdir(p):
    return os.path.abspath(os.path.expanduser(p))

def show_tree(t):
    me = t.get("self", {})
    for w in t["workspaces"]:
        kind = " (worktree)" if w.get("worktree") else ""
        branch = f" [{w['branch']}]" if w.get("branch") else ""
        print(f"workspace {w['id']}  {w['name']}{branch}{kind}  {w['path']}")
        for tab in w["tabs"]:
            print(f"  tab {tab['id']}  {tab['title'] or '-'}")
            for p in tab["panes"]:
                here = "  <- you" if p["id"] == me.get("pane") else ""
                agent = p.get("agent") or "shell"
                sess = f" session={p['session']}" if p.get("session") else ""
                print(f"    pane {p['id']}  {agent} {p.get('status') or ''}{sess}  {p['cwd']}{here}")

def main(argv):
    argv, cmd = split_cmd(argv)
    if not argv or argv[0] in ("-h", "--help", "help"):
        print(__doc__.strip())
        return
    if argv[:2] == ["workspace", "open"]:
        rest = argv[2:]
        name = take(rest, "--name")
        if len(rest) != 1:
            die("usage: beebox workspace open <path> [--name NAME] [-- CMD...]", 2)
        out = call({"op": "workspace_open", "path": absdir(rest[0]), "name": name, "run": cmd})
    elif argv[:2] == ["tab", "new"]:
        rest = argv[2:]
        ws, name, cwd = take(rest, "--ws"), take(rest, "--name"), take(rest, "--cwd")
        if rest:
            die("usage: beebox tab new [--ws ID] [--name TITLE] [--cwd DIR] [-- CMD...]", 2)
        if ws is not None and not ws.isdigit():
            die(f"--ws takes a workspace id, got {ws!r}", 2)
        out = call({"op": "tab_new", "ws": int(ws) if ws else None, "name": name,
                    "cwd": absdir(cwd) if cwd else None, "run": cmd})
    elif argv[:2] == ["workspace", "close"]:
        if len(argv) != 3 or not argv[2].isdigit():
            die("usage: beebox workspace close <ID>", 2)
        out = call({"op": "workspace_close", "ws": int(argv[2])})
    elif argv == ["check"]:
        out = call({"op": "check"})
        if not out.get("allowed"):
            die("refused — someone can type in this terminal through a Workspace/Tab/Pane "
                "link, and would not see what it opens")
    elif argv[0] == "list":
        out = call({"op": "list"})
        if "--json" not in argv:
            show_tree(out)
            return
    else:
        die(f"unknown command: {' '.join(argv)} (see beebox --help)", 2)
    print(json.dumps(out, ensure_ascii=False))

main(sys.argv[1:])
"#;

/// What a Codex session in a BeeBox pane is told on start, via the
/// SessionStart hook's `additionalContext`. Codex has no flag for an extra
/// skills directory, and its hook trust is keyed on the command string — so
/// the hook command stays exactly as it was and only the sender's output
/// changes.
const CODEX_CONTEXT: &str = "In BeeBox. Delegate: `beebox check` first (refused: stop, tell the user), then \
`beebox tab new -- codex \"<brief>\"`; a worktree only if asked. Message a session: \
`codex queue --thread <session from beebox list> --message ...`. Details: `beebox --help`.";

/// The Claude plugin's one skill. Loaded with `--plugin-dir` by the wrapper,
/// so it exists only in BeeBox panes; until used it costs Claude the
/// description line and nothing else.
const CLAUDE_SKILL: &str = r#"---
name: beebox
description: Delegate tasks to new visible Claude sessions — BeeBox tabs, or git worktrees on request. Use when the user wants work handed off or run in parallel, or a BeeBox tab/workspace opened.
---

1. `beebox check`. Refused: stop, tell the user, run nothing else (no git).
2. Your name: ListAgents → "This session is <name>".
3. Per task:
   - Default: `beebox tab new --name <n> -- claude --name <n> "<brief>"`.
   - Worktree only if asked, only in a git repo (never `git init`):
     `git worktree add <main>.worktrees/<branch> -b <branch>` (<main> = first
     line of `git worktree list`, or the project's convention); copy .env etc.,
     avoid port/DB clashes; then
     `beebox workspace open <path> -- claude --name <branch> "<brief>"`.
     It branches from HEAD; uncommitted changes stay behind — tell the user.
   - Tabs in one workspace editing the same files: mention it once.
4. Brief: goal, acceptance criteria, key files; end with "Questions or done →
   SendMessage <your name>, conclusions only."
5. Coordinate only via SendMessage; never read or type into other terminals.
6. Merge (into the branch it started from), worktree removal, branch
   deletion: ask first.

Reference: `beebox --help`.
"#;

const CLAUDE_PLUGIN_JSON: &str = r#"{
  "name": "beebox",
  "version": "1.0.0",
  "description": "BeeBox terminal: open workspaces and tabs, run work in parallel"
}
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

  # The plugin carries one skill, the `beebox` command's — so Claude knows of
  # it here and nowhere else.
  "$real" --settings "$BEEBOX_AGENT_HOOKS_DIR/claude-settings.json" \
    --plugin-dir "$BEEBOX_AGENT_HOOKS_DIR/claude-plugin" "$@"
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
# After the user's PATH, so `beebox` cannot shadow anything of theirs.
if [ -n "$BEEBOX_AGENT_HOOKS_DIR" ] && [ -d "$BEEBOX_AGENT_HOOKS_DIR/bin" ]; then
  path+=("$BEEBOX_AGENT_HOOKS_DIR/bin")
fi
# A command this pane was opened to run (`beebox tab new -- <cmd>`): typed at
# the first prompt as if by hand, so it goes through aliases and the wrappers
# above, lands in history, and leaves the shell behind when it exits. Read and
# unset at once, so a shell started inside this one does not run it again.
if [ -n "$BEEBOX_RUN" ]; then
  _beebox_run="$BEEBOX_RUN"
  unset BEEBOX_RUN
  _beebox_run_once() {
    add-zle-hook-widget -d line-init _beebox_run_once
    BUFFER="$_beebox_run"
    unset _beebox_run
    zle accept-line
  }
  autoload -Uz add-zle-hook-widget
  zle -N _beebox_run_once
  add-zle-hook-widget line-init _beebox_run_once
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
  # Anything that is now a regular file was written by Codex via rename(2).
  # Newer than the user's copy: it goes home. Older: the user changed theirs
  # since (another terminal, say), and theirs wins. Either way the overlay
  # points at the user's file again, so nothing here goes stale. hooks.json
  # is ours and stays ours. Databases are left as they were: one in use can
  # be neither copied nor swapped for a link safely.
  local item name home
  for item in "$overlay"/*(N); do
    [ -f "$item" ] || continue
    [ -L "$item" ] && continue
    name="${item:t}"
    case "$name" in
      hooks.json|hooks.json.*) continue ;;
      *.sqlite|*.sqlite-wal|*.sqlite-shm)
        mkdir -p "$user_home" 2>/dev/null || continue
        cp -f "$item" "$user_home/$name" 2>/dev/null || true
        continue ;;
    esac
    home="$user_home/$name"
    mkdir -p "$user_home" 2>/dev/null || continue
    # A tie goes to the overlay: losing a login or a trust approval made in
    # Codex is the worse mistake.
    if [ ! -e "$home" ] || ! [ "$home" -nt "$item" ]; then
      cp -f "$item" "$home" 2>/dev/null || continue
    fi
    ln -sfn "$home" "$item"
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

# Our hooks first, then the user's own: Codex reads one hooks.json, so theirs
# would otherwise never run in a BeeBox pane. Rebuilt every launch, so an edit
# to ~/.codex/hooks.json lands next time. Ours keep their exact text and stay
# first in each event — Codex keys hook trust on path, event, position and
# content, so approvals already given survive. The user's need approving once
# under this path in /hooks; until then Codex skips them, as before.
send="$BEEBOX_AGENT_HOOKS_DIR/send"
# Its own temp file per launch: panes started together (a batch of delegated
# tasks) would otherwise rename each other's half-written file into place.
python3 "$BEEBOX_AGENT_HOOKS_DIR/codex-hooks" "$send" "$user_home/hooks.json" > "$overlay/hooks.json.$$" \
  && mv -f "$overlay/hooks.json.$$" "$overlay/hooks.json" \
  || rm -f "$overlay/hooks.json.$$"

export CODEX_HOME="$overlay"
trap sync_back EXIT INT TERM

# Subprocess + wait, keeping the real exit code. `|| code=$?` stops set -e
# from skipping the trap-carrying exit path.
code=0
"$real" "$@" || code=$?
exit "$code"
"#;

/// Builds the overlay's hooks.json: BeeBox's hooks, then the user's own from
/// `~/.codex/hooks.json`. BeeBox's entries are the same entries, in the same
/// place, as before the user's were merged in — Codex's trust in them carries
/// over (checked against a real Codex). A broken or missing user file just
/// contributes nothing.
const CODEX_HOOKS: &str = r#"import json, sys

send, user_file = sys.argv[1], sys.argv[2]
EVENTS = ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "PermissionRequest", "Stop"]

def ours(event):
    return {"hooks": [{"type": "command", "command": f'"{send}" codex {event}', "timeout": 3}]}

try:
    with open(user_file) as f:
        user = json.load(f).get("hooks") or {}
    if not isinstance(user, dict):
        user = {}
except Exception:
    user = {}

def foreign(group):
    # A copy of ours in the user's file would fire twice.
    return not any(send in str(h.get("command", "")) for h in group.get("hooks", []) if isinstance(h, dict))

merged = {e: [ours(e)] for e in EVENTS}
for event, groups in user.items():
    if isinstance(groups, list):
        merged.setdefault(event, []).extend(g for g in groups if isinstance(g, dict) and foreign(g))
print(json.dumps({"hooks": merged}, indent=2, ensure_ascii=False))
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
    std::fs::write(
        hooks_dir.join("codex-context.json"),
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": CODEX_CONTEXT,
            }
        })
        .to_string(),
    )?;
    let plugin = hooks_dir.join("claude-plugin");
    std::fs::create_dir_all(plugin.join(".claude-plugin"))?;
    std::fs::create_dir_all(plugin.join("skills/beebox"))?;
    std::fs::write(plugin.join(".claude-plugin/plugin.json"), CLAUDE_PLUGIN_JSON)?;
    std::fs::write(plugin.join("skills/beebox/SKILL.md"), CLAUDE_SKILL)?;
    let bin = hooks_dir.join("bin");
    std::fs::create_dir_all(&bin)?;
    std::fs::write(bin.join("beebox"), BEEBOX_CLI)?;
    std::fs::set_permissions(bin.join("beebox"), std::fs::Permissions::from_mode(0o755))?;
    std::fs::write(hooks_dir.join("claude-wrapper.zsh"), CLAUDE_WRAPPER)?;
    std::fs::write(hooks_dir.join("codex-wrapper.zsh"), CODEX_WRAPPER)?;
    std::fs::write(hooks_dir.join("codex-hooks"), CODEX_HOOKS)?;
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
        // A counter, not the clock: macOS time has microsecond resolution, so
        // tests starting together got one directory and deleted it under
        // each other.
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "beebox-adapters-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
    fn the_beebox_command_and_its_introductions_are_installed() {
        let dir = tmp();
        let a = install(&dir.0).unwrap();
        let cli = a.hooks_dir.join("bin/beebox");
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(std::fs::metadata(&cli).unwrap().permissions().mode() & 0o111, 0);

        // Runs, explains itself, and outside a pane says so instead of
        // failing somewhere in urllib.
        let help = std::process::Command::new(&cli).arg("--help").output().unwrap();
        assert!(help.status.success(), "{}", String::from_utf8_lossy(&help.stderr));
        assert!(String::from_utf8_lossy(&help.stdout).contains("beebox workspace open"));
        let outside = std::process::Command::new(&cli)
            .arg("list")
            .env_remove("BEEBOX_CLI_URL")
            .output()
            .unwrap();
        assert_eq!(outside.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&outside.stderr).contains("not inside a BeeBox terminal"));

        // Claude learns of it through a plugin skill, Codex through context
        // its SessionStart hook prints.
        let skill = std::fs::read_to_string(a.hooks_dir.join("claude-plugin/skills/beebox/SKILL.md")).unwrap();
        assert!(skill.starts_with("---\nname: beebox\n"));
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(a.hooks_dir.join("claude-plugin/.claude-plugin/plugin.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["name"], "beebox");
        let ctx: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(a.hooks_dir.join("codex-context.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(ctx["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert!(CLAUDE_WRAPPER.contains("--plugin-dir \"$BEEBOX_AGENT_HOOKS_DIR/claude-plugin\""));
    }

    #[test]
    fn codex_hooks_keep_ours_first_and_add_the_users() {
        let dir = tmp();
        let a = install(&dir.0).unwrap();
        let send = a.hooks_dir.join("send").to_string_lossy().into_owned();
        let merge = |user: &str| -> serde_json::Value {
            let f = dir.0.join("user-hooks.json");
            std::fs::write(&f, user).unwrap();
            let out = std::process::Command::new("python3")
                .arg(a.hooks_dir.join("codex-hooks"))
                .arg(&send)
                .arg(&f)
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            serde_json::from_slice(&out.stdout).unwrap()
        };

        // Ours, exactly as Codex trusted them: same command, type, timeout.
        let ours = |v: &serde_json::Value, event: &str| v["hooks"][event][0]["hooks"][0].clone();
        let alone = merge("not json");
        for event in ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "PermissionRequest", "Stop"] {
            assert_eq!(
                ours(&alone, event),
                serde_json::json!({"type": "command", "command": format!("\"{send}\" codex {event}"), "timeout": 3}),
            );
            assert_eq!(alone["hooks"][event].as_array().unwrap().len(), 1, "a broken user file adds nothing");
        }

        let user = serde_json::json!({"hooks": {
            "Stop": [{"hooks": [{"type": "command", "command": "notify-me"}]}],
            "PostCompact": [{"hooks": [{"type": "command", "command": "log-it"}]}],
            // A copy of ours must not fire twice.
            "PreToolUse": [{"hooks": [{"type": "command", "command": format!("\"{send}\" codex PreToolUse")}]}],
        }});
        let v = merge(&user.to_string());
        assert_eq!(v["hooks"]["Stop"][1]["hooks"][0]["command"], "notify-me", "theirs come after ours");
        assert_eq!(ours(&v, "Stop")["command"], format!("\"{send}\" codex Stop"));
        assert_eq!(v["hooks"]["PostCompact"][0]["hooks"][0]["command"], "log-it", "events we do not use are kept");
        assert_eq!(v["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);

        // Odd shapes contribute nothing rather than breaking ours.
        for odd in [r#"{}"#, r#"{"hooks": []}"#, r#"{"hooks": {"Stop": "x", "PreToolUse": [1, "y"]}}"#] {
            let v = merge(odd);
            assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1, "{odd}");
            assert_eq!(v["hooks"]["PreToolUse"].as_array().unwrap().len(), 1, "{odd}");
        }
        assert!(CODEX_RUN.contains("codex-hooks\" \"$send\" \"$user_home/hooks.json\" > \"$overlay/hooks.json.$$\""));
    }

    #[test]
    fn codex_config_written_in_beebox_goes_home_unless_home_is_newer() {
        // Codex saves config by rename(2), which turns the overlay's symlink
        // into a file of its own. A fake `codex` does exactly that; for the
        // second case the user's real file is edited afterwards, as another
        // terminal would.
        for (case, expect) in [("codex", "from-codex"), ("then-user", "from-user")] {
            let dir = tmp();
            let a = install(&dir.0).unwrap();
            let home = dir.0.join("user-codex");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(home.join("config.toml"), "original").unwrap();
            let fake = dir.0.join("fake-codex");
            std::fs::write(
                &fake,
                r#"#!/bin/sh
printf from-codex > "$CODEX_HOME/config.toml.tmp" && mv -f "$CODEX_HOME/config.toml.tmp" "$CODEX_HOME/config.toml"
if [ "$1" = then-user ]; then sleep 1.1; printf from-user > "$USER_CODEX/config.toml"; fi
"#,
            )
            .unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

            let out = std::process::Command::new(a.hooks_dir.join("codex-run"))
                .arg(case)
                .env("BEEBOX_AGENT_HOOKS_DIR", &a.hooks_dir)
                .env("BEEBOX_USER_CODEX_HOME", &home)
                .env("BEEBOX_REAL_CODEX", &fake)
                .env("USER_CODEX", &home)
                .output()
                .unwrap();
            assert!(out.status.success(), "{case}: {}", String::from_utf8_lossy(&out.stderr));

            assert_eq!(std::fs::read_to_string(home.join("config.toml")).unwrap(), expect, "{case}");
            let overlay = dir.0.join("codex-overlay/config.toml");
            assert!(overlay.symlink_metadata().unwrap().file_type().is_symlink(), "{case}: linked again");
            assert_eq!(std::fs::read_to_string(&overlay).unwrap(), expect, "{case}: and reads the user's file");
        }
    }

    #[test]
    fn a_pane_command_is_typed_once_and_not_inherited() {
        // Read and unset before anything else can see it: a shell started
        // inside the pane must not run the command a second time.
        let read = ZSHRC.find("_beebox_run=\"$BEEBOX_RUN\"").unwrap();
        let unset = ZSHRC.find("unset BEEBOX_RUN").unwrap();
        assert!(read < unset);
        // After the user's own rc, so their aliases and the wrappers apply.
        assert!(ZSHRC.find("$ZDOTDIR/.zshrc").unwrap() < read);
        assert!(ZSHRC.contains("add-zle-hook-widget -d line-init _beebox_run_once"));
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
