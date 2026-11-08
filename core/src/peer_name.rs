//! Keeps a Claude session's peer name — the one other agents address it by —
//! across restarts.
//!
//! Claude derives a name for every process it starts (`<dir>-<2 hex>`) and
//! draws the suffix afresh each time, so `bee-box-17` comes back from a resume
//! as `bee-box-c2` and messages sent to the old name go nowhere. What it does
//! keep is an `agent-name` line in the transcript: `/rename` writes one, and a
//! resume reads the last one back. So the name is read from Claude's registry
//! while the process runs, and once it has exited the line is added for it —
//! only when the transcript has none, so a name the user chose always wins.
//!
//! Deliberately not `custom-title`, which is what `claude -n` would write: that
//! is the title too, and would replace the generated one in the tab, in search
//! and in Claude's own `/resume` list.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::valid_session_id;

const MARKER: &[u8] = br#""type":"agent-name""#;

/// The name Claude's registry gives the live process running `session`.
/// `~/.claude/sessions/<pid>.json`, one per process; it is removed as the
/// process exits, before `SessionEnd`, so this only answers while it runs.
pub fn lookup(session: &str) -> Option<String> {
    lookup_in(&crate::hooks::claude_data_dir()?.join("sessions"), session)
}

/// Adds `agent-name: name` to the session's transcript unless it already has
/// one. Only once the process is gone: Claude appends to the same file while
/// it runs. Returns whether a line was written.
pub fn pin(session: &str, name: &str) -> bool {
    let Some(projects) = crate::hooks::claude_data_dir().map(|d| d.join("projects")) else {
        return false;
    };
    match find_transcript(&projects, session) {
        Some(path) => pin_in(&path, session, name).unwrap_or(false),
        None => false,
    }
}

fn lookup_in(dir: &Path, session: &str) -> Option<String> {
    if !valid_session_id(session) {
        return None;
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .find_map(|p| {
            let v: Value = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            if v.get("sessionId").and_then(Value::as_str) != Some(session) {
                return None;
            }
            v.get("name").and_then(Value::as_str).filter(|n| valid_name(n)).map(str::to_string)
        })
}

/// The name goes back into Claude's own file, so only what Claude itself
/// would have put there: short, one line.
fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.chars().count() <= 64 && !n.chars().any(char::is_control)
}

/// `<projects>/<mangled cwd>/<session>.jsonl`; looking in each project is
/// cheaper than reproducing the mangling.
pub(crate) fn find_transcript(projects: &Path, session: &str) -> Option<PathBuf> {
    if !valid_session_id(session) {
        return None;
    }
    let name = format!("{session}.jsonl");
    std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|e| e.path().join(&name))
        .filter(|p| p.is_file())
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
}

fn pin_in(path: &Path, session: &str, name: &str) -> std::io::Result<bool> {
    if !valid_session_id(session) || !valid_name(name) {
        return Ok(false);
    }
    let Some(last) = scan(path)? else { return Ok(false) };
    let line = serde_json::json!({ "type": "agent-name", "agentName": name, "sessionId": session });
    let mut out = Vec::new();
    // A transcript cut off mid-line must not swallow ours into it.
    if last.is_some_and(|b| b != b'\n') {
        out.push(b'\n');
    }
    out.extend_from_slice(line.to_string().as_bytes());
    out.push(b'\n');
    std::fs::OpenOptions::new().append(true).open(path)?.write_all(&out)?;
    Ok(true)
}

/// Looks for an `agent-name` line without holding the file in memory: the
/// sessions worth pinning are the long ones, and those run past 100 MB.
/// `None` if there is one; otherwise the file's last byte.
fn scan(path: &Path) -> std::io::Result<Option<Option<u8>>> {
    let finder = memchr::memmem::Finder::new(MARKER);
    let mut file = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    // Carried over between reads so a marker split across two is still seen.
    let keep = MARKER.len() - 1;
    let mut filled = 0;
    let mut last = None;
    loop {
        let n = file.read(&mut buf[filled..])?;
        if n == 0 {
            return Ok(Some(last));
        }
        filled += n;
        last = Some(buf[filled - 1]);
        if finder.find(&buf[..filled]).is_some() {
            return Ok(None);
        }
        let tail = filled.saturating_sub(keep);
        buf.copy_within(tail..filled, 0);
        filled -= tail;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "beebox-peer-{tag}-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_registry_entry_for_the_session_gives_the_name() {
        let d = tmp("reg");
        std::fs::write(d.join("1.json"), r#"{"pid":1,"sessionId":"other","name":"x-01"}"#).unwrap();
        std::fs::write(d.join("2.json"), r#"{"pid":2,"sessionId":"s-1","name":"bee-box-17","nameSource":"derived"}"#)
            .unwrap();
        std::fs::write(d.join("2.abc.key"), "not json").unwrap();
        assert_eq!(lookup_in(&d, "s-1").as_deref(), Some("bee-box-17"));
        assert_eq!(lookup_in(&d, "gone"), None);
        assert_eq!(lookup_in(&d, "../s-1"), None);
    }

    #[test]
    fn pins_once_and_never_over_a_name_already_there() {
        let d = tmp("pin");
        let t = d.join("s-1.jsonl");
        std::fs::write(&t, "{\"type\":\"user\"}\n{\"type\":\"ai-title\",\"aiTitle\":\"Fix it\"}").unwrap();

        assert!(pin_in(&t, "s-1", "bee-box-17").unwrap());
        let text = std::fs::read_to_string(&t).unwrap();
        let last: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(last["type"], "agent-name");
        assert_eq!(last["agentName"], "bee-box-17");
        assert_eq!(last["sessionId"], "s-1");
        // The unterminated line before it is left whole.
        assert_eq!(text.lines().nth(1), Some(r#"{"type":"ai-title","aiTitle":"Fix it"}"#));
        assert!(!text.contains("custom-title"), "the title is not touched");

        // A second exit, or a resume after one, writes nothing more.
        assert!(!pin_in(&t, "s-1", "bee-box-c2").unwrap());
        assert_eq!(std::fs::read_to_string(&t).unwrap(), text);
    }

    #[test]
    fn a_name_far_into_a_long_transcript_is_found() {
        let d = tmp("long");
        let t = d.join("s-1.jsonl");
        // Past the first read, and straddling a read boundary.
        let mut body = vec![b'x'; (1 << 20) - 5];
        body.extend_from_slice(b"{\"type\":\"agent-name\",\"agentName\":\"mine\"}\n");
        std::fs::write(&t, &body).unwrap();
        assert!(!pin_in(&t, "s-1", "bee-box-17").unwrap());
        assert_eq!(std::fs::read(&t).unwrap(), body);
    }

    #[test]
    fn a_rename_by_the_user_is_left_alone() {
        let d = tmp("user");
        let t = d.join("s-1.jsonl");
        let body = "{\"type\":\"custom-title\",\"customTitle\":\"zsh-fix\"}\n{\"type\":\"agent-name\",\"agentName\":\"zsh-fix\"}\n";
        std::fs::write(&t, body).unwrap();
        assert!(!pin_in(&t, "s-1", "bee-box-17").unwrap());
        assert_eq!(std::fs::read_to_string(&t).unwrap(), body);
    }

    #[test]
    fn finds_the_transcript_in_any_project() {
        let d = tmp("find");
        std::fs::create_dir_all(d.join("-Users-me-a")).unwrap();
        std::fs::create_dir_all(d.join("-Users-me-b")).unwrap();
        std::fs::write(d.join("-Users-me-b/s-1.jsonl"), "{}\n").unwrap();
        assert_eq!(find_transcript(&d, "s-1"), Some(d.join("-Users-me-b/s-1.jsonl")));
        assert_eq!(find_transcript(&d, "s-2"), None);
    }

    #[test]
    fn odd_names_are_refused() {
        assert!(valid_name("bee-box-17"));
        assert!(valid_name("修 zsh"));
        assert!(!valid_name(""));
        assert!(!valid_name("a\nb"));
        assert!(!valid_name(&"x".repeat(65)));
    }
}
