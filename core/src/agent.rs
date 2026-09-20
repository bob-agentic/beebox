//! Agent status domain model: the six-phase state machine that hook events
//! drive, and the text rules for what those events may put on screen.
//!
//! This is the single source of truth for state transitions. The HTTP layer
//! normalizes raw agent payloads into `AgentEvent` and everything after that
//! happens here, where it is testable without a socket or a process.
//!
//! Two things this module is *not*: it never inspects terminal output (the
//! product boundary — bytes pass through, hooks are the only agent signal),
//! and it never equates a PTY exit with a turn result. `failed` means a
//! structured tool error inside a turn; process exits stay `Out::Exited`.

use serde_json::Value;

use crate::proto::{AgentEvent, AgentEventKind, AgentKind, AgentPhase, AgentStatusView};

/// Runtime agent state for one pane. Wraps the wire view with the one bit of
/// turn context that must not go over the wire: whether any tool has failed
/// since the last prompt.
#[derive(Debug, Clone)]
pub struct AgentState {
    view: AgentStatusView,
    /// Sticky within a turn: one failing tool makes the whole turn `failed`,
    /// no matter how many succeed after it. Cleared by the next prompt.
    turn_had_error: bool,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            view: AgentStatusView {
                phase: AgentPhase::NeverRan,
                agent: None,
                revision: 0,
                at_ms: 0,
                started_at_ms: None,
                tool_detail: None,
                summary: None,
            },
            turn_had_error: false,
        }
    }
}

impl AgentState {
    pub fn view(&self) -> &AgentStatusView {
        &self.view
    }

    /// Applies one normalized event. Returns `true` if the view changed and
    /// should be broadcast.
    ///
    /// Ordering: concurrent hook connections can deliver events out of order,
    /// so anything older than what is already shown is dropped — a stale
    /// `running` must never overwrite a newer `success`.
    pub fn apply(&mut self, ev: &AgentEvent) -> bool {
        if ev.at_ms < self.view.at_ms {
            return false;
        }

        let accepted = match &ev.kind {
            AgentEventKind::SessionStart => {
                // A fresh session is idle — but a completion dot from the
                // previous turn stays until the next prompt, so restarting the
                // agent does not silently eat an unread result.
                self.turn_had_error = false;
                if matches!(self.view.phase, AgentPhase::Success | AgentPhase::Failed) {
                    false
                } else {
                    self.view.phase = AgentPhase::Idle;
                    self.view.tool_detail = None;
                    true
                }
            }
            AgentEventKind::PromptSubmit => {
                // New turn: everything from the previous one is stale.
                self.view.phase = AgentPhase::Running;
                self.view.started_at_ms = Some(ev.at_ms);
                self.view.tool_detail = None;
                self.view.summary = None;
                self.turn_had_error = false;
                true
            }
            AgentEventKind::PreTool { detail } => {
                self.view.phase = AgentPhase::Running;
                self.view.tool_detail = detail.clone();
                // A tool without a preceding prompt (resumed session) still
                // starts the clock; within a turn the start never resets.
                if self.view.started_at_ms.is_none() {
                    self.view.started_at_ms = Some(ev.at_ms);
                }
                true
            }
            AgentEventKind::PostTool { failed } => {
                self.turn_had_error |= failed;
                // Releases needs_input: a permission prompt that has been
                // answered is over the moment the tool completes.
                self.view.phase = AgentPhase::Running;
                if self.view.started_at_ms.is_none() {
                    self.view.started_at_ms = Some(ev.at_ms);
                }
                true
            }
            AgentEventKind::NeedsInput => {
                // Only reachable from running. Claude also emits Notification
                // as an idle heartbeat 60s after a turn ends — that must not
                // overwrite success/failed.
                if self.view.phase == AgentPhase::Running {
                    self.view.phase = AgentPhase::NeedsInput;
                    true
                } else {
                    false
                }
            }
            AgentEventKind::TurnEnd { summary } => {
                self.view.phase = if self.turn_had_error {
                    AgentPhase::Failed
                } else {
                    AgentPhase::Success
                };
                self.view.summary = summary.clone();
                self.view.tool_detail = None;
                true
            }
            AgentEventKind::SessionEnd => {
                // Idle, but never over a finished turn's result.
                if matches!(self.view.phase, AgentPhase::Success | AgentPhase::Failed) {
                    false
                } else {
                    self.view.phase = AgentPhase::Idle;
                    self.view.tool_detail = None;
                    true
                }
            }
        };

        if accepted {
            self.view.agent = Some(ev.agent);
            self.view.at_ms = ev.at_ms;
            self.view.revision += 1;
        }
        accepted
    }
}

/// Aggregation order for tab and workspace roll-ups. Fixed by the spec:
/// the most actionable state wins.
pub const PHASE_PRIORITY: [AgentPhase; 6] = [
    AgentPhase::NeedsInput,
    AgentPhase::Running,
    AgentPhase::Failed,
    AgentPhase::Success,
    AgentPhase::Idle,
    AgentPhase::NeverRan,
];

pub fn rollup<'a>(phases: impl Iterator<Item = &'a AgentPhase>) -> AgentPhase {
    let present: std::collections::HashSet<AgentPhase> = phases.copied().collect();
    PHASE_PRIORITY
        .into_iter()
        .find(|p| present.contains(p))
        .unwrap_or(AgentPhase::NeverRan)
}

// ---- text hygiene ---------------------------------------------------------

/// Everything a hook may put on screen goes through here: control characters
/// (including newlines and ANSI escapes) become spaces, runs collapse, and the
/// result is capped in *characters*, not bytes — a UTF-8 boundary cut would
/// corrupt CJK text.
pub fn sanitize(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut last_space = true; // leading spaces are dropped
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        // Swallow a whole ANSI escape sequence, not just the ESC byte —
        // otherwise "\x1b[31m" leaves "[31m" in a tooltip.
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                // CSI: parameters then one final byte in @..~.
                for f in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&f) {
                        break;
                    }
                }
            }
            continue;
        }
        let c = if c.is_control() { ' ' } else { c };
        if c == ' ' {
            if last_space {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        out.push(c);
        if out.chars().count() >= max_chars {
            break;
        }
    }
    out.trim_end().to_string()
}

/// Longest strings the UI will ever be handed.
pub const MAX_SUMMARY_CHARS: usize = 200;
pub const MAX_TITLE_CHARS: usize = 200;
const MAX_DETAIL_CHARS: usize = 60;

/// The last `n` segments of a path — enough to recognise a file without
/// leaking the whole tree into a tooltip.
fn last_segments(path: &str, n: usize) -> String {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let start = parts.len().saturating_sub(n);
    parts[start..].join("/")
}

/// One line of tool context for the tooltip. Per-tool rules from the spec;
/// anything unknown shows its name and nothing else.
pub fn tool_detail(tool: &str, input: &Value) -> String {
    let detail = match tool {
        "Edit" | "Write" | "Read" | "MultiEdit" | "NotebookEdit" => input
            .get("file_path")
            .or_else(|| input.get("notebook_path"))
            .and_then(Value::as_str)
            .map(|p| sanitize(&last_segments(p, 3), MAX_DETAIL_CHARS)),
        "Bash" => input
            .get("command")
            .and_then(Value::as_str)
            .and_then(|c| c.lines().next())
            .map(|l| sanitize(l, MAX_DETAIL_CHARS)),
        "Grep" | "Glob" => input
            .get("pattern")
            .and_then(Value::as_str)
            .map(|p| sanitize(p, MAX_DETAIL_CHARS)),
        "Task" => input
            .get("subagent_type")
            .and_then(Value::as_str)
            .map(|s| sanitize(s, MAX_DETAIL_CHARS)),
        _ => None,
    };
    let tool = sanitize(tool, 40);
    match detail {
        Some(d) if !d.is_empty() => format!("{tool} {d}"),
        _ => tool,
    }
}

/// Whitelist for agent session ids. These end up in a resume command line, so
/// nothing outside this alphabet is ever stored.
pub fn valid_session_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn parse_agent_kind(s: &str) -> Option<AgentKind> {
    match s {
        "claude" => Some(AgentKind::Claude),
        "opencode" => Some(AgentKind::Opencode),
        "codex" => Some(AgentKind::Codex),
        _ => None,
    }
}

pub fn agent_kind_str(k: AgentKind) -> &'static str {
    match k {
        AgentKind::Claude => "claude",
        AgentKind::Opencode => "opencode",
        AgentKind::Codex => "codex",
    }
}

/// The resume command line for one agent session. Built from a trusted enum
/// plus a whitelist-validated id — never from stored shell text. `None` when
/// the id fails the whitelist, so a hand-edited database row cannot inject.
/// Mirrors mux0's `resume_command_for`.
pub fn resume_command(agent: AgentKind, session_id: &str) -> Option<String> {
    if !valid_session_id(session_id) {
        return None;
    }
    Some(match agent {
        AgentKind::Claude => format!("claude --resume {session_id}"),
        AgentKind::Codex => format!("codex resume {session_id}"),
        AgentKind::Opencode => format!("opencode --session {session_id}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: AgentEventKind, at_ms: i64) -> AgentEvent {
        AgentEvent {
            agent: AgentKind::Claude,
            kind,
            at_ms,
            session_id: None,
            session_title: None,
        }
    }

    fn pre(at: i64) -> AgentEvent {
        ev(AgentEventKind::PreTool { detail: Some("Edit src/auth.rs".into()) }, at)
    }

    #[test]
    fn a_full_turn_with_a_tool_error_ends_failed() {
        let mut s = AgentState::default();
        assert!(s.apply(&ev(AgentEventKind::PromptSubmit, 10)));
        assert_eq!(s.view().phase, AgentPhase::Running);
        assert!(s.apply(&pre(20)));
        assert!(s.apply(&ev(AgentEventKind::PostTool { failed: true }, 30)));
        assert!(s.apply(&ev(AgentEventKind::TurnEnd { summary: None }, 40)));
        assert_eq!(s.view().phase, AgentPhase::Failed);
    }

    #[test]
    fn one_tool_error_sticks_for_the_whole_turn() {
        let mut s = AgentState::default();
        s.apply(&ev(AgentEventKind::PromptSubmit, 10));
        s.apply(&ev(AgentEventKind::PostTool { failed: true }, 20));
        // Later tools succeed; the turn is still failed.
        s.apply(&ev(AgentEventKind::PostTool { failed: false }, 30));
        s.apply(&ev(AgentEventKind::PostTool { failed: false }, 40));
        s.apply(&ev(AgentEventKind::TurnEnd { summary: None }, 50));
        assert_eq!(s.view().phase, AgentPhase::Failed);
    }

    #[test]
    fn the_next_prompt_clears_the_previous_turns_error() {
        let mut s = AgentState::default();
        s.apply(&ev(AgentEventKind::PromptSubmit, 10));
        s.apply(&ev(AgentEventKind::PostTool { failed: true }, 20));
        s.apply(&ev(AgentEventKind::TurnEnd { summary: None }, 30));
        assert_eq!(s.view().phase, AgentPhase::Failed);

        s.apply(&ev(AgentEventKind::PromptSubmit, 40));
        assert_eq!(s.view().phase, AgentPhase::Running);
        assert!(s.view().summary.is_none(), "old summary must not linger");
        s.apply(&ev(AgentEventKind::TurnEnd { summary: None }, 50));
        assert_eq!(s.view().phase, AgentPhase::Success);
    }

    #[test]
    fn post_tool_releases_needs_input() {
        let mut s = AgentState::default();
        s.apply(&ev(AgentEventKind::PromptSubmit, 10));
        s.apply(&ev(AgentEventKind::NeedsInput, 20));
        assert_eq!(s.view().phase, AgentPhase::NeedsInput);
        s.apply(&ev(AgentEventKind::PostTool { failed: false }, 30));
        assert_eq!(s.view().phase, AgentPhase::Running);
    }

    #[test]
    fn needs_input_only_enters_from_running() {
        let mut s = AgentState::default();
        // From never_ran: refused.
        assert!(!s.apply(&ev(AgentEventKind::NeedsInput, 10)));
        assert_eq!(s.view().phase, AgentPhase::NeverRan);

        // The idle-heartbeat case: after a finished turn, refused.
        s.apply(&ev(AgentEventKind::PromptSubmit, 20));
        s.apply(&ev(AgentEventKind::TurnEnd { summary: None }, 30));
        assert!(!s.apply(&ev(AgentEventKind::NeedsInput, 40)));
        assert_eq!(s.view().phase, AgentPhase::Success);
    }

    #[test]
    fn idle_never_overwrites_a_turn_result() {
        let mut s = AgentState::default();
        s.apply(&ev(AgentEventKind::PromptSubmit, 10));
        s.apply(&ev(AgentEventKind::TurnEnd { summary: Some("done".into()) }, 20));
        assert!(!s.apply(&ev(AgentEventKind::SessionEnd, 30)));
        assert_eq!(s.view().phase, AgentPhase::Success);
        assert_eq!(s.view().summary.as_deref(), Some("done"));

        // SessionStart is guarded the same way: a restart must not eat an
        // unread result.
        assert!(!s.apply(&ev(AgentEventKind::SessionStart, 40)));
        assert_eq!(s.view().phase, AgentPhase::Success);
    }

    #[test]
    fn stale_events_never_move_the_state_backwards() {
        let mut s = AgentState::default();
        s.apply(&ev(AgentEventKind::PromptSubmit, 100));
        s.apply(&ev(AgentEventKind::TurnEnd { summary: None }, 200));
        // A delayed pre_tool from the finished turn arrives late.
        assert!(!s.apply(&pre(150)));
        assert_eq!(s.view().phase, AgentPhase::Success);
    }

    #[test]
    fn started_at_survives_multiple_tools_in_one_turn() {
        let mut s = AgentState::default();
        s.apply(&ev(AgentEventKind::PromptSubmit, 100));
        s.apply(&pre(200));
        s.apply(&ev(AgentEventKind::PostTool { failed: false }, 300));
        s.apply(&pre(400));
        assert_eq!(s.view().started_at_ms, Some(100));
        // And it is still there after the turn, for the duration tooltip.
        s.apply(&ev(AgentEventKind::TurnEnd { summary: None }, 500));
        assert_eq!(s.view().started_at_ms, Some(100));
    }

    #[test]
    fn every_accepted_event_bumps_the_revision() {
        let mut s = AgentState::default();
        assert_eq!(s.view().revision, 0);
        s.apply(&ev(AgentEventKind::PromptSubmit, 10));
        let r1 = s.view().revision;
        s.apply(&pre(20));
        assert!(s.view().revision > r1);
        // A refused event must not bump it.
        let r2 = s.view().revision;
        assert!(!s.apply(&ev(AgentEventKind::NeedsInput, 5)));
        assert_eq!(s.view().revision, r2);
    }

    #[test]
    fn session_start_is_idle_and_clears_tool_detail() {
        let mut s = AgentState::default();
        assert!(s.apply(&ev(AgentEventKind::SessionStart, 10)));
        assert_eq!(s.view().phase, AgentPhase::Idle);
        assert_eq!(s.view().agent, Some(AgentKind::Claude));
    }

    #[test]
    fn rollup_priority_is_fixed() {
        use AgentPhase::*;
        assert_eq!(rollup([Idle, Success, NeedsInput, Running].iter()), NeedsInput);
        assert_eq!(rollup([Idle, Success, Running].iter()), Running);
        assert_eq!(rollup([Idle, Success, Failed].iter()), Failed);
        assert_eq!(rollup([Idle, Success].iter()), Success);
        assert_eq!(rollup([NeverRan, Idle].iter()), Idle);
        assert_eq!(rollup([].iter()), NeverRan);
    }

    // ---- text hygiene ----

    #[test]
    fn sanitize_strips_control_chars_and_caps_by_chars() {
        assert_eq!(sanitize("a\x1b[31mb\nc", 100), "ab c");
        // Caps in characters, not bytes: CJK must not be cut mid-codepoint.
        let s = sanitize(&"你".repeat(300), MAX_SUMMARY_CHARS);
        assert_eq!(s.chars().count(), MAX_SUMMARY_CHARS);
    }

    #[test]
    fn tool_detail_follows_the_per_tool_rules() {
        let j = |s: &str| serde_json::from_str::<Value>(s).unwrap();
        assert_eq!(
            tool_detail("Edit", &j(r#"{"file_path":"/very/deep/repo/src/auth.rs"}"#)),
            "Edit repo/src/auth.rs"
        );
        assert_eq!(
            tool_detail("Bash", &j(r#"{"command":"echo hi\nrm -rf /"}"#)),
            "Bash echo hi",
            "only the first line"
        );
        let long = format!(r#"{{"command":"{}"}}"#, "x".repeat(200));
        assert!(tool_detail("Bash", &j(&long)).chars().count() <= 65);
        assert_eq!(tool_detail("Grep", &j(r#"{"pattern":"fn main"}"#)), "Grep fn main");
        assert_eq!(tool_detail("Task", &j(r#"{"subagent_type":"Explore"}"#)), "Task Explore");
        // Unknown tools show only their name — and the name is sanitized too.
        assert_eq!(tool_detail("My\x07Tool", &j(r#"{"x":1}"#)), "My Tool");
    }

    #[test]
    fn session_id_whitelist() {
        assert!(valid_session_id("abc-123_XYZ"));
        assert!(!valid_session_id(""));
        assert!(!valid_session_id("a;rm -rf /"));
        assert!(!valid_session_id(&"a".repeat(129)));
        assert!(!valid_session_id("a b"));
    }

    #[test]
    fn resume_commands_come_from_the_enum_and_a_validated_id() {
        assert_eq!(
            resume_command(AgentKind::Claude, "abc-123").as_deref(),
            Some("claude --resume abc-123")
        );
        assert_eq!(
            resume_command(AgentKind::Codex, "abc-123").as_deref(),
            Some("codex resume abc-123")
        );
        // A malformed id yields nothing — not a sanitized command, nothing.
        assert!(resume_command(AgentKind::Claude, "x; rm -rf /").is_none());
        assert!(resume_command(AgentKind::Claude, "").is_none());
    }
}
