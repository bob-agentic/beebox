//! Trusted hook ingest: raw agent payload → one normalized `AgentEvent`.
//!
//! Normalization happens here and nowhere else. The state machine in
//! `agent.rs` consumes `AgentEvent` and never sees agent-specific JSON, so a
//! new adapter (OpenCode, Codex) is a new `normalize_*` function plus
//! fixtures — not new match arms scattered through the daemon.
//!
//! Everything in this file treats its input as hostile: the hook port may be
//! exposed anywhere the user chose, and even a legitimate agent runs
//! arbitrary code. Strings are sanitized, session ids whitelisted, and
//! transcript paths confined to the agent's own data directory.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::{
    sanitize, tool_detail, valid_session_id, MAX_SUMMARY_CHARS, MAX_TITLE_CHARS,
};
use crate::proto::{AgentEvent, AgentEventKind, AgentKind};

/// Hook bodies are tiny; anything bigger is not a hook.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Reduces one raw Claude Code hook payload to a normalized event.
/// `None` means "not something we track" — the endpoint still answers 204,
/// because an unknown event must never break the agent.
pub fn normalize_claude(body: &Value, at_ms: i64) -> Option<AgentEvent> {
    let event = body.get("hook_event_name")?.as_str()?;
    let session_id = body
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| valid_session_id(s))
        .map(str::to_string);

    let kind = match event {
        "SessionStart" => AgentEventKind::SessionStart,
        "UserPromptSubmit" => AgentEventKind::PromptSubmit,
        "PreToolUse" => {
            let tool = body.get("tool_name").and_then(Value::as_str).unwrap_or("tool");
            let input = body.get("tool_input").cloned().unwrap_or(Value::Null);
            AgentEventKind::PreTool { detail: Some(tool_detail(tool, &input)) }
        }
        "PostToolUse" => AgentEventKind::PostTool {
            failed: claude_tool_failed(body.get("tool_response").unwrap_or(&Value::Null)),
        },
        // The amber dot. The state machine only lets this in from `running`,
        // which is what keeps Claude's 60s idle heartbeat from overwriting a
        // finished turn.
        "Notification" => AgentEventKind::NeedsInput,
        "Stop" => {
            // The payload itself carries the turn's last assistant message —
            // use it first: at Stop time the transcript may not be flushed
            // yet (observed in the real-agent smoke test). The transcript is
            // still read for the session title (ai-title / custom-title).
            let inline = body
                .get("last_assistant_message")
                .and_then(Value::as_str)
                .map(|s| sanitize(s, MAX_SUMMARY_CHARS))
                .filter(|s| !s.is_empty());
            let meta = body
                .get("transcript_path")
                .and_then(Value::as_str)
                .and_then(|p| read_claude_transcript(Path::new(p)));
            let (file_summary, title) = meta.unwrap_or_default();
            return Some(AgentEvent {
                agent: AgentKind::Claude,
                kind: AgentEventKind::TurnEnd { summary: inline.or(file_summary) },
                at_ms,
                session_id,
                session_title: title,
            });
        }
        "SessionEnd" => AgentEventKind::SessionEnd,
        _ => return None,
    };

    // The first prompt doubles as the title fallback until the transcript
    // offers an ai-title; empty and slash-command prompts are not names.
    let session_title = if event == "UserPromptSubmit" {
        body.get("prompt")
            .and_then(Value::as_str)
            .map(|p| sanitize(p, MAX_TITLE_CHARS))
            .filter(|p| !p.is_empty() && !p.starts_with('/'))
    } else {
        None
    };

    Some(AgentEvent { agent: AgentKind::Claude, kind, at_ms, session_id, session_title })
}

/// A turn is `failed` when a tool reported a structured error. This never
/// looks at exit codes of the agent process — only at what the agent itself
/// said about its tool call.
fn claude_tool_failed(resp: &Value) -> bool {
    match resp {
        Value::Object(o) => {
            o.get("is_error").and_then(Value::as_bool).unwrap_or(false)
                || o.get("success").and_then(Value::as_bool) == Some(false)
                || o.get("error").is_some_and(|e| !e.is_null())
        }
        Value::Array(items) => items.iter().any(claude_tool_failed),
        _ => false,
    }
}

/// Reduces one Codex hook payload (wrapped by `send codex <event>`) to a
/// normalized event. Codex's hook payloads use the same field names as
/// Claude's (session_id / tool_name / tool_input / tool_response) — mux0's
/// dispatcher relies on the same fact. Titles come from the session rollout,
/// not the payload; `PermissionRequest` is the amber dot.
pub fn normalize_codex(body: &Value, at_ms: i64) -> Option<AgentEvent> {
    let event = body.get("hook_event_name")?.as_str()?;
    let session_id = body
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| valid_session_id(s))
        .map(str::to_string);

    let kind = match event {
        "SessionStart" => AgentEventKind::SessionStart,
        "UserPromptSubmit" => AgentEventKind::PromptSubmit,
        "PreToolUse" => {
            let tool = body.get("tool_name").and_then(Value::as_str).unwrap_or("tool");
            let input = body.get("tool_input").cloned().unwrap_or(Value::Null);
            AgentEventKind::PreTool { detail: Some(tool_detail(tool, &input)) }
        }
        "PostToolUse" => AgentEventKind::PostTool {
            failed: claude_tool_failed(body.get("tool_response").unwrap_or(&Value::Null)),
        },
        "PermissionRequest" => AgentEventKind::NeedsInput,
        "Stop" => AgentEventKind::TurnEnd {
            summary: body
                .get("last_assistant_message")
                .and_then(Value::as_str)
                .map(|s| sanitize(s, MAX_SUMMARY_CHARS))
                .filter(|s| !s.is_empty()),
        },
        _ => return None,
    };

    // Title priority (mux0's read_codex_title): rollout thread_name_updated
    // beats the first user message; the prompt in this payload is the
    // last-resort fallback. Read on prompt and turn end, when it can change.
    let session_title = match event {
        "UserPromptSubmit" | "Stop" => {
            let from_rollout = session_id.as_deref().and_then(read_codex_rollout_title);
            from_rollout.or_else(|| {
                body.get("prompt")
                    .and_then(Value::as_str)
                    .map(|p| sanitize(p, MAX_TITLE_CHARS))
                    .filter(|p| !p.is_empty() && !p.starts_with('/'))
            })
        }
        _ => None,
    };

    Some(AgentEvent { agent: AgentKind::Codex, kind, at_ms, session_id, session_title })
}

/// Rollouts this daemon is willing to open. Same confinement as Claude
/// transcripts, against the real user CODEX_HOME (the overlay symlinks
/// `sessions/` back here).
fn codex_sessions_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".codex").join("sessions"))
}

/// Finds `rollout-*-<session_id>.jsonl` under the nested year/month/day tree
/// and reads its title: `thread_name_updated.thread_name` beats the first
/// `user_message` (mux0's read_codex_title, re-implemented).
fn read_codex_rollout_title(session_id: &str) -> Option<String> {
    if !valid_session_id(session_id) {
        return None;
    }
    let dir = codex_sessions_dir()?.canonicalize().ok()?;
    let path = find_rollout(&dir, session_id, 0)?;
    let text = std::fs::read_to_string(path).ok()?;

    let mut thread_name: Option<String> = None;
    let mut first_prompt: Option<String> = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("type").and_then(Value::as_str) != Some("event_msg") {
            continue;
        }
        let Some(p) = v.get("payload") else { continue };
        match p.get("type").and_then(Value::as_str) {
            Some("thread_name_updated") => {
                thread_name = p
                    .get("thread_name")
                    .and_then(Value::as_str)
                    .map(|t| sanitize(t, MAX_TITLE_CHARS))
                    .filter(|t| !t.is_empty());
            }
            Some("user_message") if first_prompt.is_none() => {
                first_prompt = p
                    .get("message")
                    .and_then(Value::as_str)
                    .map(|t| sanitize(t, MAX_TITLE_CHARS))
                    .filter(|t| !t.is_empty());
            }
            _ => {}
        }
    }
    thread_name.or(first_prompt)
}

/// Walks the sessions tree (bounded depth: year/month/day) for the newest
/// rollout matching the session id. The id is already whitelist-validated,
/// so the suffix match cannot be a glob or path trick.
fn find_rollout(dir: &Path, session_id: &str, depth: usize) -> Option<PathBuf> {
    if depth > 3 {
        return None;
    }
    let suffix = format!("-{session_id}.jsonl");
    let mut best: Option<PathBuf> = None;
    for entry in std::fs::read_dir(dir).ok()? {
        let Ok(e) = entry else { continue };
        let p = e.path();
        let ft = e.file_type().ok()?;
        if ft.is_dir() {
            if let Some(hit) = find_rollout(&p, session_id, depth + 1) {
                if best.as_ref().is_none_or(|b| hit > *b) {
                    best = Some(hit);
                }
            }
        } else if ft.is_file() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("rollout-") && name.ends_with(&suffix)
                && best.as_ref().is_none_or(|b| p > *b)
            {
                best = Some(p);
            }
        }
    }
    best
}

/// Transcripts this daemon is willing to open. Confinement, not trust: the
/// path arrived over HTTP, so "wherever it points" is not an option.
fn claude_data_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude"))
}

/// Pulls (summary, title) out of a Claude transcript. Summary is the last
/// assistant text; title prefers `/rename` (custom-title) over the generated
/// ai-title. Both sanitized and capped before they can reach a tooltip.
fn read_claude_transcript(path: &Path) -> Option<(Option<String>, Option<String>)> {
    let dir = claude_data_dir()?;
    // Canonicalize both sides so a symlink cannot step out of the directory.
    let path = path.canonicalize().ok()?;
    let dir = dir.canonicalize().ok()?;
    if !path.starts_with(&dir) || path.extension().is_none_or(|e| e != "jsonl") {
        return None;
    }
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > 64 * 1024 * 1024 {
        return None;
    }

    let text = std::fs::read_to_string(&path).ok()?;
    let mut summary: Option<String> = None;
    let mut ai_title: Option<String> = None;
    let mut custom_title: Option<String> = None;

    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                // The last text block of the last assistant message.
                if let Some(items) = v.pointer("/message/content").and_then(Value::as_array) {
                    let t = items
                        .iter()
                        .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
                        .filter_map(|c| c.get("text").and_then(Value::as_str))
                        .last();
                    if let Some(t) = t {
                        let s = sanitize(t, MAX_SUMMARY_CHARS);
                        if !s.is_empty() {
                            summary = Some(s);
                        }
                    }
                }
            }
            Some("ai-title") => {
                ai_title = v
                    .get("aiTitle")
                    .and_then(Value::as_str)
                    .map(|t| sanitize(t, MAX_TITLE_CHARS));
            }
            Some("custom-title") => {
                custom_title = v
                    .get("customTitle")
                    .and_then(Value::as_str)
                    .map(|t| sanitize(t, MAX_TITLE_CHARS));
            }
            _ => {}
        }
    }
    let title = custom_title.or(ai_title).filter(|t| !t.is_empty());
    Some((summary, title))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn j(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn every_claude_event_maps_to_its_kind() {
        let cases = [
            (r#"{"hook_event_name":"SessionStart","session_id":"s-1"}"#, AgentEventKind::SessionStart),
            (r#"{"hook_event_name":"UserPromptSubmit","session_id":"s-1","prompt":"fix the bug"}"#, AgentEventKind::PromptSubmit),
            (r#"{"hook_event_name":"Notification","message":"needs permission"}"#, AgentEventKind::NeedsInput),
            (r#"{"hook_event_name":"SessionEnd"}"#, AgentEventKind::SessionEnd),
        ];
        for (body, want) in cases {
            let ev = normalize_claude(&j(body), 1).expect(body);
            assert_eq!(ev.kind, want, "{body}");
            assert_eq!(ev.agent, AgentKind::Claude);
        }
    }

    #[test]
    fn pre_tool_carries_a_sanitized_detail() {
        let ev = normalize_claude(
            &j(r#"{"hook_event_name":"PreToolUse","tool_name":"Edit",
                   "tool_input":{"file_path":"/a/b/c/d/auth.rs"}}"#),
            1,
        )
        .unwrap();
        assert_eq!(
            ev.kind,
            AgentEventKind::PreTool { detail: Some("Edit c/d/auth.rs".into()) },
            "the last three path segments"
        );
    }

    #[test]
    fn post_tool_error_shapes_are_recognised() {
        for resp in [
            r#"{"is_error":true}"#,
            r#"{"success":false}"#,
            r#"{"error":"boom"}"#,
            r#"[{"type":"text"},{"is_error":true}]"#,
        ] {
            let body = format!(
                r#"{{"hook_event_name":"PostToolUse","tool_response":{resp}}}"#
            );
            let ev = normalize_claude(&j(&body), 1).unwrap();
            assert_eq!(ev.kind, AgentEventKind::PostTool { failed: true }, "{resp}");
        }
        let ok = normalize_claude(
            &j(r#"{"hook_event_name":"PostToolUse","tool_response":{"output":"fine"}}"#),
            1,
        )
        .unwrap();
        assert_eq!(ok.kind, AgentEventKind::PostTool { failed: false });
    }

    #[test]
    fn unknown_events_are_ignored_not_errors() {
        assert!(normalize_claude(&j(r#"{"hook_event_name":"PreCompact"}"#), 1).is_none());
        assert!(normalize_claude(&j(r#"{"something":"else"}"#), 1).is_none());
    }

    #[test]
    fn bad_session_ids_are_dropped_not_stored() {
        let ev = normalize_claude(
            &j(r#"{"hook_event_name":"SessionStart","session_id":"x; rm -rf /"}"#),
            1,
        )
        .unwrap();
        assert!(ev.session_id.is_none());
    }

    #[test]
    fn slash_command_prompts_do_not_become_titles() {
        let ev = normalize_claude(
            &j(r#"{"hook_event_name":"UserPromptSubmit","prompt":"/model"}"#),
            1,
        )
        .unwrap();
        assert!(ev.session_title.is_none());
        let ev = normalize_claude(
            &j(r#"{"hook_event_name":"UserPromptSubmit","prompt":"fix login\nplease"}"#),
            1,
        )
        .unwrap();
        assert_eq!(ev.session_title.as_deref(), Some("fix login please"));
    }

    #[test]
    fn transcript_paths_outside_the_claude_dir_are_refused() {
        // /etc/passwd exists but is not ours to read on a hook's say-so.
        assert!(read_claude_transcript(Path::new("/etc/passwd")).is_none());
        assert!(read_claude_transcript(Path::new("/nonexistent/x.jsonl")).is_none());
    }

    #[test]
    fn stop_prefers_the_inline_last_assistant_message() {
        // Real Claude sends the summary in the payload itself; the transcript
        // can lag behind the Stop hook.
        let ev = normalize_claude(
            &j(r#"{"hook_event_name":"Stop","last_assistant_message":"All done.\nTests pass."}"#),
            1,
        )
        .unwrap();
        assert_eq!(
            ev.kind,
            AgentEventKind::TurnEnd { summary: Some("All done. Tests pass.".into()) }
        );
    }

    #[test]
    fn codex_events_map_like_claudes_with_permission_request_as_amber() {
        let cases = [
            (r#"{"hook_event_name":"SessionStart","session_id":"c-1"}"#, AgentEventKind::SessionStart),
            (r#"{"hook_event_name":"UserPromptSubmit","session_id":"c-1","prompt":"fix it"}"#, AgentEventKind::PromptSubmit),
            (r#"{"hook_event_name":"PermissionRequest"}"#, AgentEventKind::NeedsInput),
        ];
        for (body, want) in cases {
            let ev = normalize_codex(&j(body), 1).expect(body);
            assert_eq!(ev.kind, want, "{body}");
            assert_eq!(ev.agent, AgentKind::Codex);
        }

        let pre = normalize_codex(
            &j(r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"make"}}"#),
            1,
        )
        .unwrap();
        assert_eq!(pre.kind, AgentEventKind::PreTool { detail: Some("Bash make".into()) });

        let post = normalize_codex(
            &j(r#"{"hook_event_name":"PostToolUse","tool_response":{"is_error":true}}"#),
            1,
        )
        .unwrap();
        assert_eq!(post.kind, AgentEventKind::PostTool { failed: true });

        // Unknown codex events (Notification does not exist there) are ignored.
        assert!(normalize_codex(&j(r#"{"hook_event_name":"Notification"}"#), 1).is_none());
    }

    #[test]
    fn codex_prompt_falls_back_to_the_prompt_when_no_rollout_exists() {
        let ev = normalize_codex(
            &j(r#"{"hook_event_name":"UserPromptSubmit","session_id":"no-such-rollout-xyz","prompt":"build the parser"}"#),
            1,
        )
        .unwrap();
        assert_eq!(ev.session_title.as_deref(), Some("build the parser"));
    }
}

