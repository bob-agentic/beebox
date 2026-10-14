//! Finding words in past Claude conversations, and reading one back whole.
//!
//! Plain text matching over `~/.claude/projects/*/<session>.jsonl` — no index,
//! nothing kept between searches. A transcript is mostly tool output, so the
//! raw bytes are scanned for the words first and only the lines they land on
//! are parsed; then the match must survive into what was actually said, or a
//! word that only appeared in a tool's output would bring back nothing to show.
//!
//! The raw scan is `memmem`, once per spelling of each word, rather than one
//! case-insensitive automaton: measured on 1.5 GB of real transcripts, the
//! automaton ran at about 500 MB/s and `memmem` at about 25 GB/s.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use aho_corasick::AhoCorasick;
use memchr::memmem::Finder;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::valid_session_id;

/// Who a message is from. `Tool` is one line per tool call: what it touched,
/// not what it returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Who {
    User,
    Ai,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConvMsg {
    /// The transcript line's uuid, with the block number for tool calls. What
    /// a hit and the same message in the whole conversation agree on.
    pub key: String,
    pub who: Who,
    pub at_ms: i64,
    pub text: String,
}

/// One conversation with words in it. `hits` are snippets, newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub session: String,
    pub title: String,
    pub cwd: String,
    pub first_ms: i64,
    pub last_ms: i64,
    /// Every hit, though `hits` carries at most `MAX_HITS_PER_SESSION`.
    pub total: u32,
    pub hits: Vec<ConvMsg>,
}

/// Where to look: every project, or sessions started under one of these folders.
pub enum Where {
    All,
    Under(Vec<String>),
}

const MAX_WORDS: usize = 8;
const MAX_WORD_CHARS: usize = 64;
const MAX_SESSIONS: usize = 100;
const MAX_HITS_PER_SESSION: usize = 50;
/// The largest transcript read. Real ones run to tens of MB.
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const SNIPPET_BEFORE: usize = 40;
const SNIPPET_CHARS: usize = 240;
/// A message in a whole conversation. Long answers are kept long: this is for
/// reading, but a pasted log should not make the frame tens of MB.
const MAX_MSG_CHARS: usize = 8000;
const MAX_TOOL_CHARS: usize = 160;

/// The words of a query: space-separated, any one matching is a hit.
pub fn words(q: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in q.split_whitespace() {
        let w: String = w.chars().take(MAX_WORD_CHARS).collect();
        if !out.iter().any(|x| x.eq_ignore_ascii_case(&w)) {
            out.push(w);
        }
        if out.len() == MAX_WORDS {
            break;
        }
    }
    out
}

pub fn search(words: &[String], scope: &Where) -> Vec<Found> {
    let Some(projects) = projects_dir() else { return Vec::new() };
    search_in(&projects, words, scope)
}

fn search_in(projects: &Path, words: &[String], scope: &Where) -> Vec<Found> {
    if words.is_empty() {
        return Vec::new();
    }
    let Ok(ac) = AhoCorasick::builder().ascii_case_insensitive(true).build(words) else {
        return Vec::new();
    };
    let needles = Needles::new(words);
    let files = transcripts(projects, scope);

    // Files are independent and mostly already in the page cache: one per
    // core at a time.
    let next = AtomicUsize::new(0);
    let found = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(files.len().max(1));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(path) = files.get(i) else { break };
                if let Some(f) = scan(path, &needles, &ac, scope) {
                    found.lock().unwrap().push(f);
                }
            });
        }
    });

    let mut found = dedupe(found.into_inner().unwrap());
    let newest = |f: &Found| f.hits.first().map_or(0, |h| h.at_ms);
    found.sort_by_key(|f| std::cmp::Reverse(newest(f)));
    found.truncate(MAX_SESSIONS);
    for f in &mut found {
        f.hits.truncate(MAX_HITS_PER_SESSION);
    }
    found
}

/// A resumed conversation can start a new transcript that repeats the old
/// one's messages, uuids and all, so one thing said shows up in both. It is
/// kept in the transcript that went on longest — the one a resume would pick
/// up — and a transcript left with nothing of its own drops out.
fn dedupe(mut found: Vec<Found>) -> Vec<Found> {
    found.sort_by_key(|f| std::cmp::Reverse(f.last_ms));
    let mut seen = std::collections::HashSet::new();
    for f in &mut found {
        f.hits.retain(|h| seen.insert(h.key.clone()));
        f.total = f.hits.len() as u32;
    }
    found.retain(|f| !f.hits.is_empty());
    found
}

/// Every message of a session, oldest first. `None` when there is no such
/// transcript.
pub fn conversation(session: &str) -> Option<Vec<ConvMsg>> {
    let path = transcript_of(session)?;
    let buf = read(&path)?;
    let mut out = Vec::new();
    for line in buf.split(|&b| b == b'\n') {
        if let Ok(v) = serde_json::from_slice::<Value>(line) {
            out.extend(messages(&v, MAX_MSG_CHARS));
        }
    }
    Some(out)
}

/// The folder a session ran in, as its transcript recorded it.
pub fn session_cwd(session: &str) -> Option<String> {
    let buf = read(&transcript_of(session)?)?;
    meta(&buf).cwd
}

/// `~/.claude/projects/<project>/<session>.jsonl`, wherever it was started.
pub fn transcript_of(session: &str) -> Option<PathBuf> {
    if !valid_session_id(session) {
        return None;
    }
    let name = format!("{session}.jsonl");
    std::fs::read_dir(projects_dir()?)
        .ok()?
        .flatten()
        .map(|e| e.path().join(&name))
        .filter(|p| p.is_file())
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
}

fn projects_dir() -> Option<PathBuf> {
    crate::hooks::claude_data_dir().map(|d| d.join("projects"))
}

/// Claude names a project's folder after its path, every character other
/// than a letter or digit turned into `-`.
fn mangle(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Whether `cwd` is one of `roots` or inside one.
fn within(cwd: &str, roots: &[String]) -> bool {
    roots.iter().any(|r| {
        let r = r.trim_end_matches('/');
        cwd == r || cwd.strip_prefix(r).is_some_and(|rest| rest.starts_with('/'))
    })
}

/// The transcripts worth opening. The folder name is only a first cut — a
/// sibling `app-old` mangles to a prefix of `app` — so `scan` checks the cwd
/// the transcript itself recorded.
fn transcripts(projects: &Path, scope: &Where) -> Vec<PathBuf> {
    let prefixes: Option<Vec<String>> = match scope {
        Where::All => None,
        Where::Under(roots) => Some(roots.iter().map(|r| mangle(r.trim_end_matches('/'))).collect()),
    };
    let Ok(dirs) = std::fs::read_dir(projects) else { return Vec::new() };
    let mut out = Vec::new();
    for d in dirs.flatten() {
        let name = d.file_name().to_string_lossy().into_owned();
        if let Some(ps) = &prefixes {
            if !ps.iter().any(|p| name == *p || name.starts_with(&format!("{p}-"))) {
                continue;
            }
        }
        // Top level only: subagents keep theirs in a folder per session.
        let Ok(files) = std::fs::read_dir(d.path()) else { continue };
        out.extend(
            files
                .flatten()
                .map(|f| f.path())
                .filter(|p| p.extension().is_some_and(|e| e == "jsonl") && p.is_file()),
        );
    }
    out
}

fn read(path: &Path) -> Option<Vec<u8>> {
    let len = path.metadata().ok()?.len();
    if len > MAX_FILE_BYTES {
        return None;
    }
    std::fs::read(path).ok()
}

/// The spellings a word is looked for in raw bytes: as typed, lower, upper and
/// capitalised. Mixed case like `ToolTip` from a lowercase query is missed; a
/// query typed that way still finds it.
struct Needles(Vec<Finder<'static>>);

impl Needles {
    fn new(words: &[String]) -> Self {
        let mut spellings: Vec<String> = Vec::new();
        for w in words {
            let mut cap = w.to_ascii_lowercase();
            if let Some(first) = cap.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            for s in [w.clone(), w.to_ascii_lowercase(), w.to_ascii_uppercase(), cap] {
                // As the transcript stores it: a `"` or `\` in the word is
                // escaped in the JSON, so the bare word would never be found.
                let s = serde_json::to_string(&s).map_or(s, |j| j[1..j.len() - 1].to_string());
                if !spellings.contains(&s) {
                    spellings.push(s);
                }
            }
        }
        Self(spellings.iter().map(|s| Finder::new(s.as_bytes()).into_owned()).collect())
    }

    fn cursor<'a>(&'a self, buf: &'a [u8]) -> Cursor<'a> {
        Cursor { needles: self, buf, next: self.0.iter().map(|f| f.find(buf)).collect() }
    }
}

/// Walks the matches in one buffer, earliest first.
struct Cursor<'a> {
    needles: &'a Needles,
    buf: &'a [u8],
    /// Where each spelling next matches, `None` once it has no more. Kept, so
    /// a rare spelling is not searched for again from wherever a common one
    /// stopped.
    next: Vec<Option<usize>>,
}

impl Cursor<'_> {
    /// The first match at or after `from`, as `(start, end)`.
    fn find(&mut self, from: usize) -> Option<(usize, usize)> {
        let mut best: Option<(usize, usize)> = None;
        for (f, next) in self.needles.0.iter().zip(self.next.iter_mut()) {
            if next.is_some_and(|at| at < from) {
                *next = self.buf.get(from..).and_then(|rest| f.find(rest)).map(|j| from + j);
            }
            if let Some(at) = *next {
                if best.is_none_or(|(b, _)| at < b) {
                    best = Some((at, at + f.needle().len()));
                }
            }
        }
        best
    }
}

fn scan(path: &Path, needles: &Needles, ac: &AhoCorasick, scope: &Where) -> Option<Found> {
    let buf = read(path)?;
    let mut cursor = needles.cursor(&buf);
    // Most files have none of the words: done after one pass.
    let mut next = cursor.find(0);
    next?;
    let m = meta(&buf);
    let cwd = m.cwd.clone().unwrap_or_default();
    if let Where::Under(roots) = scope {
        if !within(&cwd, roots) {
            return None;
        }
    }

    let mut hits = Vec::new();
    while let Some((at, after)) = next {
        let start = memchr::memrchr(b'\n', &buf[..at]).map_or(0, |i| i + 1);
        let end = memchr::memchr(b'\n', &buf[after..]).map_or(buf.len(), |i| after + i);
        // The rest of this line's matches would only parse it again.
        next = cursor.find(end);
        let line = &buf[start..end];
        // Parsing is the cost, and the lines a word lands on are mostly tool
        // results and attachments, megabytes each, that say nothing to show.
        if !said_something(line) {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else { continue };
        for msg in messages(&v, usize::MAX) {
            if let Some(at) = ac.find(msg.text.as_bytes()) {
                hits.push(ConvMsg { text: snippet(&msg.text, at.start()), ..msg });
            }
        }
    }
    if hits.is_empty() {
        return None;
    }
    hits.sort_by_key(|h| std::cmp::Reverse(h.at_ms));
    let total = hits.len() as u32;

    let session = path.file_stem()?.to_string_lossy().into_owned();
    Some(Found {
        session,
        title: m.title.unwrap_or_default(),
        cwd,
        first_ms: m.first_ms,
        last_ms: m.last_ms,
        total,
        hits,
    })
}

/// A cheap look at a raw line: a prompt or an answer, and not a tool's
/// result (which Claude files under the user).
fn said_something(line: &[u8]) -> bool {
    let has = |n: &[u8]| memchr::memmem::find(line, n).is_some();
    (has(b"\"type\":\"user\"") || has(b"\"type\":\"assistant\"")) && !has(b"\"toolUseResult\"")
}

#[derive(Default)]
struct Meta {
    cwd: Option<String>,
    title: Option<String>,
    first_ms: i64,
    last_ms: i64,
}

/// What a conversation is called, where it ran and when. Only the lines that
/// can say so are parsed.
fn meta(buf: &[u8]) -> Meta {
    let lines = || buf.split(|&b| b == b'\n').filter(|l| !l.is_empty());
    let parse = |l: &[u8]| serde_json::from_slice::<Value>(l).ok();
    let has = |l: &[u8], needle: &[u8]| memchr::memmem::find(l, needle).is_some();

    let mut m = Meta::default();
    for l in lines() {
        if m.cwd.is_some() && m.first_ms != 0 {
            break;
        }
        let Some(v) = parse(l) else { continue };
        if m.cwd.is_none() {
            m.cwd = v.get("cwd").and_then(Value::as_str).map(str::to_string);
        }
        if m.first_ms == 0 {
            m.first_ms = timestamp(&v).unwrap_or(0);
        }
    }
    for l in buf.rsplit(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        if let Some(t) = parse(l).as_ref().and_then(timestamp) {
            m.last_ms = t;
            break;
        }
    }

    // `/rename` over the generated title, the last of each; the first thing
    // asked when there is neither.
    let (mut custom, mut ai) = (None, None);
    for l in lines().filter(|l| has(l, b"-title\"")) {
        let Some(v) = parse(l) else { continue };
        match v.get("type").and_then(Value::as_str) {
            Some("custom-title") => custom = v.get("customTitle").and_then(Value::as_str).map(str::to_string),
            Some("ai-title") => ai = v.get("aiTitle").and_then(Value::as_str).map(str::to_string),
            _ => {}
        }
    }
    m.title = custom.or(ai).filter(|t| !t.trim().is_empty()).or_else(|| {
        lines()
            .filter(|l| has(l, b"\"type\":\"user\""))
            .filter_map(parse)
            .flat_map(|v| messages(&v, 120))
            .find(|msg| msg.who == Who::User)
            .map(|msg| flat(&msg.text))
    });
    m
}

/// What a transcript line said, as messages. Prompts and answers keep their
/// text; a tool call becomes one line naming what it touched. Tool results,
/// thinking, system notes and subagents' sidechains are left out.
fn messages(v: &Value, max_chars: usize) -> Vec<ConvMsg> {
    let kind = v.get("type").and_then(Value::as_str);
    if !matches!(kind, Some("user" | "assistant"))
        || v.get("isMeta").and_then(Value::as_bool) == Some(true)
        || v.get("isSidechain").and_then(Value::as_bool) == Some(true)
    {
        return Vec::new();
    }
    let Some(uuid) = v.get("uuid").and_then(Value::as_str) else { return Vec::new() };
    let at_ms = timestamp(v).unwrap_or(0);
    let content = v.pointer("/message/content");
    let said = |who: Who, text: String| -> Option<ConvMsg> {
        let text = text.trim();
        // `<command-name>`, `<local-command-stdout>`, `<system-reminder>`:
        // the CLI talking to itself under the user's name.
        if text.is_empty() || text.starts_with('<') {
            return None;
        }
        Some(ConvMsg { key: uuid.to_string(), who, at_ms, text: cap(text, max_chars) })
    };

    if kind == Some("user") {
        let text = match content {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(blocks)) => texts(blocks),
            _ => return Vec::new(),
        };
        return said(Who::User, text).into_iter().collect();
    }

    let Some(Value::Array(blocks)) = content else { return Vec::new() };
    let mut out: Vec<ConvMsg> = said(Who::Ai, texts(blocks)).into_iter().collect();
    for (i, b) in blocks.iter().enumerate() {
        if b.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let name = b.get("name").and_then(Value::as_str).unwrap_or("tool");
        let input = b.get("input");
        let arg = ["file_path", "command", "pattern", "url", "query", "description"]
            .iter()
            .find_map(|k| input.and_then(|i| i.get(*k)).and_then(Value::as_str))
            .unwrap_or("");
        out.push(ConvMsg {
            key: format!("{uuid}:{i}"),
            who: Who::Tool,
            at_ms,
            text: cap(&flat(&format!("{name}  {arg}")), MAX_TOOL_CHARS.min(max_chars)),
        });
    }
    out
}

fn texts(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn cap(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// One line, whitespace runs folded.
fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The part of `text` around byte `at`: a little before the match, so it is
/// never cut off at the edge of the row.
fn snippet(text: &str, at: usize) -> String {
    let before = &text[..at];
    let lead: Vec<(usize, char)> = before.char_indices().collect();
    let (start, cut) = if lead.len() > SNIPPET_BEFORE {
        (lead[lead.len() - SNIPPET_BEFORE].0, true)
    } else {
        (0, false)
    };
    let body = flat(&cap(&text[start..], SNIPPET_CHARS));
    if cut { format!("…{body}") } else { body }
}

fn timestamp(v: &Value) -> Option<i64> {
    v.get("timestamp").and_then(Value::as_str).and_then(parse_rfc3339)
}

/// `2026-10-13T05:01:09.268Z` to milliseconds since the epoch. Claude writes
/// UTC with a `Z` and nothing else.
fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || !s.ends_with('Z') {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (h, mi, sec) = (n(11..13)?, n(14..16)?, n(17..19)?);
    let ms = match s.get(19..20) {
        Some(".") => {
            let frac = &s[20..s.len() - 1];
            let digits: String = frac.chars().chain("000".chars()).take(3).collect();
            digits.parse::<i64>().ok()?
        }
        _ => 0,
    };
    // Days from civil, after Howard Hinnant.
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + mi) * 60_000 + sec * 1000 + ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(uuid: &str, ts: &str, text: &str) -> String {
        serde_json::json!({"type":"user","uuid":uuid,"timestamp":ts,"cwd":"/w/app",
            "message":{"role":"user","content":text}}).to_string()
    }
    fn ai(uuid: &str, ts: &str, text: &str) -> String {
        serde_json::json!({"type":"assistant","uuid":uuid,"timestamp":ts,"cwd":"/w/app",
            "message":{"role":"assistant","content":[{"type":"text","text":text}]}}).to_string()
    }

    /// A scratch `projects` folder, removed on drop.
    struct Projects(PathBuf);
    impl Projects {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("beebox-search-{}-{}", std::process::id(), rand::random::<u64>()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn put(&self, project: &str, session: &str, lines: &[String]) {
            let d = self.0.join(project);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(format!("{session}.jsonl")), lines.join("\n") + "\n").unwrap();
        }
    }
    impl Drop for Projects {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn any_word_matches_in_what_was_said_newest_first() {
        let p = Projects::new();
        p.put("-w-app", "s1", &[
            serde_json::json!({"type":"ai-title","aiTitle":"Table tooltips"}).to_string(),
            user("u1", "2026-10-13T05:01:00.000Z", "不要用默认 hover 提示，统一用 Tooltips"),
            ai("a1", "2026-10-13T05:02:00.000Z", "好，做成全局的"),
            ai("a2", "2026-10-13T05:10:00.000Z", "全局表格截断 tooltip 已经做完"),
        ]);
        let found = search_in(&p.0, &words("tooltip 悬停"), &Where::All);
        assert_eq!(found.len(), 1);
        let f = &found[0];
        assert_eq!((f.title.as_str(), f.cwd.as_str(), f.total), ("Table tooltips", "/w/app", 2));
        assert_eq!(f.hits.iter().map(|h| h.key.as_str()).collect::<Vec<_>>(), ["a2", "u1"], "newest first");
        assert_eq!(f.hits[1].who, Who::User);
        assert_eq!(f.first_ms, parse_rfc3339("2026-10-13T05:01:00.000Z").unwrap());
        assert_eq!(f.last_ms, parse_rfc3339("2026-10-13T05:10:00.000Z").unwrap());
    }

    #[test]
    fn a_word_only_in_tool_output_or_system_notes_is_not_a_hit() {
        let p = Projects::new();
        let result = serde_json::json!({"type":"user","uuid":"r","timestamp":"2026-10-13T05:01:00Z",
            "message":{"content":[{"type":"tool_result","content":"tooltip.tsx"}]}}).to_string();
        let meta = serde_json::json!({"type":"user","uuid":"m","isMeta":true,"timestamp":"2026-10-13T05:01:00Z",
            "message":{"content":"tooltip"}}).to_string();
        p.put("-w-app", "s1", &[result, meta, user("c", "2026-10-13T05:01:00Z", "<command-name>tooltip</command-name>")]);
        assert!(search_in(&p.0, &words("tooltip"), &Where::All).is_empty());
    }

    #[test]
    fn a_tool_call_is_found_by_what_it_touched() {
        let p = Projects::new();
        let call = serde_json::json!({"type":"assistant","uuid":"t","timestamp":"2026-10-13T05:01:00Z",
            "message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"/w/app/hover-card.tsx","old_string":"x"}}]}}).to_string();
        p.put("-w-app", "s1", &[call]);
        let found = search_in(&p.0, &words("hover-card"), &Where::All);
        assert_eq!(found[0].hits[0].who, Who::Tool);
        assert_eq!(found[0].hits[0].text, "Edit /w/app/hover-card.tsx");
        assert_eq!(found[0].hits[0].key, "t:0");
    }

    #[test]
    fn a_folder_scope_keeps_its_worktrees_and_drops_lookalike_siblings() {
        let p = Projects::new();
        let at = |cwd: &str| serde_json::json!({"type":"user","uuid":cwd,"timestamp":"2026-10-13T05:01:00Z",
            "cwd":cwd,"message":{"content":"tooltip"}}).to_string();
        p.put("-w-app", "main", &[at("/w/app")]);
        p.put("-w-app--claude-worktrees-x", "wt", &[at("/w/app/.claude/worktrees/x")]);
        p.put("-w-app-old", "old", &[at("/w/app-old")]);
        p.put("-w-other", "other", &[at("/w/other")]);
        let mut got: Vec<String> = search_in(&p.0, &words("tooltip"), &Where::Under(vec!["/w/app".into()]))
            .into_iter()
            .map(|f| f.session)
            .collect();
        got.sort();
        assert_eq!(got, ["main", "wt"]);
        assert_eq!(search_in(&p.0, &words("tooltip"), &Where::All).len(), 4);
    }

    #[test]
    fn a_resumed_transcript_repeating_the_old_one_is_shown_once() {
        let p = Projects::new();
        let said = user("u1", "2026-10-13T05:01:00Z", "tooltip please");
        p.put("-w-app", "old", std::slice::from_ref(&said));
        p.put("-w-app", "new", &[said, user("u2", "2026-10-14T05:01:00Z", "tooltip again")]);
        let found = search_in(&p.0, &words("tooltip"), &Where::All);
        assert_eq!(found.len(), 1, "the old transcript has nothing of its own");
        assert_eq!((found[0].session.as_str(), found[0].total), ("new", 2));
    }

    #[test]
    fn words_with_quotes_or_backslashes_are_found_as_stored() {
        let p = Projects::new();
        p.put("-w-app", "s1", &[user("u1", "2026-10-13T05:01:00Z", r#"set title="x" in C:\tmp"#)]);
        assert_eq!(search_in(&p.0, &words(r#"title="x""#), &Where::All).len(), 1);
        assert_eq!(search_in(&p.0, &words(r"C:\tmp"), &Where::All).len(), 1);
    }

    #[test]
    fn the_title_falls_back_to_the_first_prompt() {
        let lines = [user("u1", "2026-10-13T05:01:00Z", "fix the\n  login bug"), ai("a", "2026-10-13T05:02:00Z", "ok")];
        assert_eq!(meta(lines.join("\n").as_bytes()).title.as_deref(), Some("fix the login bug"));
    }

    #[test]
    fn snippets_keep_some_lead_and_fold_lines() {
        let long = format!("{}\n关键 tooltip 在这", "前".repeat(100));
        let at = long.find("tooltip").unwrap();
        let s = snippet(&long, at);
        assert!(s.starts_with('…'), "{s}");
        assert!(s.contains("关键 tooltip 在这"));
        assert_eq!(snippet("short tooltip", 6), "short tooltip");
    }

    #[test]
    fn words_dedupe_and_are_bounded() {
        assert_eq!(words("  Tooltip tooltip  悬停 "), ["Tooltip", "悬停"]);
        assert_eq!(words(&"a ".repeat(20)).len(), 1);
        assert_eq!(words(&(0..20).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ")).len(), MAX_WORDS);
    }

    #[test]
    fn timestamps_parse_to_epoch_ms() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-10-13T05:01:09.268Z"), Some(1_791_867_669_268));
        assert_eq!(parse_rfc3339("2026-10-13 05:01:09"), None);
    }

    #[test]
    fn a_folder_matches_itself_and_what_is_under_it() {
        let roots = vec!["/w/app".to_string()];
        assert!(within("/w/app", &roots));
        assert!(within("/w/app/sub", &roots));
        assert!(!within("/w/app-old", &roots));
        assert_eq!(mangle("/w/app/.claude/x_y"), "-w-app--claude-x-y");
    }
}
