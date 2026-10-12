//! The images behind `[Image #N]`, the tag Claude Code and Codex print where
//! an image was pasted into a prompt.
//!
//! The session transcript is the only place either agent keeps them: Claude's
//! copies under /tmp get cleaned away, and Codex deletes its clipboard file
//! once the message is sent. Transcripts run to hundreds of MB, though only a
//! handful of their lines hold images — so each one is indexed once, by byte
//! offset, and after that only what was appended gets read. Nothing but the
//! line holding the image asked for is ever decoded.
//!
//! The numbering differs. Claude counts across the whole session, so `#14` is
//! one image. Codex starts every message at `#1`, so the caller passes the
//! terminal row the tag was on, and the message whose text it is part of is
//! the one meant.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use serde_json::Value;

use crate::agent::valid_session_id;
use crate::proto::{AgentKind, PaneId};

/// Decoded images kept ready, oldest dropped first. A screenshot is a few
/// hundred KB, so this holds the dozens one session is likely to revisit.
const CACHE_BYTES: usize = 64 << 20;
/// Sessions remembered per pane: `/clear` starts a new one, and the images
/// of the last are still on screen above it.
const SESSIONS_PER_PANE: usize = 4;
/// Transcript indexes kept, the least recently used dropped past this. One
/// dropped is only read again — tens of ms for even a large transcript.
const INDEXES: usize = 16;

pub struct Image {
    pub mime: String,
    pub data: Vec<u8>,
}

/// Where an image is: the transcript line holding it, and which of the
/// line's images it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Loc {
    offset: u64,
    len: usize,
    nth: usize,
}

/// A Codex user message: its text, without tags or whitespace, to match a
/// terminal row against, and its images by number.
struct Msg {
    text: String,
    imgs: Vec<(u32, Loc)>,
}

#[derive(Default)]
struct Index {
    /// How far the file has been read. Only whole lines count: the agent may
    /// be halfway through writing the last one.
    scanned: u64,
    claude: HashMap<u32, Loc>,
    codex: Vec<Msg>,
}

/// A session a hook named: whose, which, and when it was last heard of.
type Seen = (AgentKind, String, i64);

#[derive(Default)]
pub struct Images {
    /// Each pane's agent sessions, newest first, with when each was last
    /// heard of — hooks land out of order.
    sessions: Mutex<HashMap<PaneId, Vec<Seen>>>,
    files: Mutex<Files>,
}

#[derive(Default)]
struct Files {
    paths: HashMap<(AgentKind, String), PathBuf>,
    indexes: HashMap<PathBuf, Index>,
    /// The indexes' transcripts, least recently used first.
    used: VecDeque<PathBuf>,
    cache: VecDeque<((PathBuf, u64, usize), Arc<Image>)>,
    cached: usize,
}

impl Images {
    /// A hook named this session for this pane.
    pub fn note_session(&self, pane: PaneId, agent: AgentKind, id: &str, at_ms: i64) {
        if !matches!(agent, AgentKind::Claude | AgentKind::Codex) || !valid_session_id(id) {
            return;
        }
        let mut all = self.sessions.lock().unwrap();
        let list = all.entry(pane).or_default();
        match list.iter_mut().find(|(a, s, _)| *a == agent && s == id) {
            Some(known) => known.2 = known.2.max(at_ms),
            None => list.push((agent, id.to_string(), at_ms)),
        }
        list.sort_by(|a, b| b.2.cmp(&a.2));
        list.truncate(SESSIONS_PER_PANE);
    }

    pub fn forget_pane(&self, pane: PaneId) {
        self.sessions.lock().unwrap().remove(&pane);
    }

    /// The sessions to look in, newest first. `stored` is the id kept for
    /// resume, for a pane no hook has spoken for since the daemon started.
    pub fn sessions_of(&self, pane: PaneId, stored: Option<(AgentKind, String)>) -> Vec<(AgentKind, String)> {
        let mut out: Vec<(AgentKind, String)> = self
            .sessions
            .lock()
            .unwrap()
            .get(&pane)
            .map(|l| l.iter().map(|(a, s, _)| (*a, s.clone())).collect())
            .unwrap_or_default();
        if let Some(s) = stored {
            if !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }

    /// Image `#n` of a session. `row` is the terminal row the tag was on,
    /// which picks the message for Codex.
    pub fn lookup(&self, agent: AgentKind, session: &str, n: u32, row: &str) -> Option<Arc<Image>> {
        let mut files = self.files.lock().unwrap();
        let path = files.path(agent, session)?;
        files.touch(&path);
        let index = files.indexes.entry(path.clone()).or_default();
        if let Err(e) = update(index, &path, agent) {
            tracing::debug!("image index {}: {e}", path.display());
        }
        let loc = match agent {
            AgentKind::Claude => index.claude.get(&n).copied(),
            AgentKind::Codex => pick_codex(&index.codex, n, row),
            AgentKind::Opencode => None,
        }?;
        let key = (path, loc.offset, loc.nth);
        if let Some((_, img)) = files.cache.iter().find(|(k, _)| *k == key) {
            return Some(img.clone());
        }
        let img = Arc::new(read_image(&key.0, loc, agent)?);
        files.cached += img.data.len();
        files.cache.push_back((key, img.clone()));
        while files.cached > CACHE_BYTES && files.cache.len() > 1 {
            if let Some((_, old)) = files.cache.pop_front() {
                files.cached -= old.data.len();
            }
        }
        Some(img)
    }
}

impl Files {
    /// Marks a transcript used, and lets go of the index used longest ago
    /// once there are more than `INDEXES` — with its path, so a session that
    /// has ended leaves nothing behind.
    fn touch(&mut self, path: &Path) {
        self.used.retain(|p| p != path);
        self.used.push_back(path.to_path_buf());
        while self.used.len() > INDEXES {
            if let Some(old) = self.used.pop_front() {
                self.indexes.remove(&old);
                self.paths.retain(|_, p| *p != old);
            }
        }
    }

    /// The session's transcript, found once and kept while it exists.
    fn path(&mut self, agent: AgentKind, session: &str) -> Option<PathBuf> {
        let key = (agent, session.to_string());
        if let Some(p) = self.paths.get(&key).filter(|p| p.is_file()) {
            return Some(p.clone());
        }
        let found = match agent {
            AgentKind::Claude => find_claude_transcript(session),
            AgentKind::Codex => {
                let dir = crate::hooks::codex_sessions_dir()?.canonicalize().ok()?;
                crate::hooks::find_rollout(&dir, session, 0)
            }
            AgentKind::Opencode => None,
        }?;
        self.paths.insert(key, found.clone());
        Some(found)
    }
}

/// `~/.claude/projects/<project>/<session>.jsonl`. The project directory is
/// the cwd the session started in, mangled; looking in each is cheaper than
/// reproducing the mangling.
fn find_claude_transcript(session: &str) -> Option<PathBuf> {
    if !valid_session_id(session) {
        return None;
    }
    let projects = crate::hooks::claude_data_dir()?.join("projects");
    let name = format!("{session}.jsonl");
    std::fs::read_dir(&projects)
        .ok()?
        .flatten()
        .map(|e| e.path().join(&name))
        .filter(|p| p.is_file())
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
}

/// Reads what was appended since last time. A line is parsed only if it
/// mentions an image at all, which is a few in a thousand.
fn update(index: &mut Index, path: &Path, agent: AgentKind) -> std::io::Result<()> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len < index.scanned {
        *index = Index::default();
    }
    if len == index.scanned {
        return Ok(());
    }
    file.seek(SeekFrom::Start(index.scanned))?;
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let marker = memchr::memmem::Finder::new(match agent {
        AgentKind::Claude => &b"\"type\":\"image\""[..],
        _ => &b"\"input_image\""[..],
    });
    let mut line = Vec::new();
    let mut at = index.scanned;
    loop {
        line.clear();
        let n = reader.read_until(b'\n', &mut line)?;
        if n == 0 || line.last() != Some(&b'\n') {
            break;
        }
        let offset = at;
        at += n as u64;
        if marker.find(&line).is_none() {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<Value>(&line) else { continue };
        match agent {
            AgentKind::Claude => index_claude(index, &v, offset, n),
            _ => index_codex(index, &v, offset, n),
        }
    }
    index.scanned = at;
    Ok(())
}

/// The content blocks of a Claude user prompt: a message, or a prompt queued
/// while the agent was busy.
fn claude_blocks(v: &Value) -> Option<&Vec<Value>> {
    if v.get("type").and_then(Value::as_str) == Some("user") {
        if let Some(c) = v.pointer("/message/content").and_then(Value::as_array) {
            return Some(c);
        }
    }
    v.pointer("/attachment/prompt").and_then(Value::as_array)
}

fn is_claude_image(b: &&Value) -> bool {
    b.get("type").and_then(Value::as_str) == Some("image")
        && b.pointer("/source/type").and_then(Value::as_str) == Some("base64")
}

/// A prompt's images follow its text, in the order they were pasted — and
/// the numbering only ever goes up, so they are the highest tags in it. Any
/// lower one is the prompt mentioning an image from earlier.
fn index_claude(index: &mut Index, v: &Value, offset: u64, len: usize) {
    let Some(blocks) = claude_blocks(v) else { return };
    let images = blocks.iter().filter(is_claude_image).count();
    if images == 0 {
        return;
    }
    let mut nums: Vec<u32> = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .flat_map(tags)
        .collect();
    nums.sort_unstable();
    nums.dedup();
    for (nth, n) in (0..images).rev().zip(nums.into_iter().rev()) {
        index.claude.insert(n, Loc { offset, len, nth });
    }
}

/// Codex puts each image between `<image name=[Image #N] …>` and `</image>`,
/// then the prompt as typed.
fn index_codex(index: &mut Index, v: &Value, offset: u64, len: usize) {
    if v.get("type").and_then(Value::as_str) != Some("response_item") {
        return;
    }
    let Some(p) = v.get("payload") else { return };
    if p.get("type").and_then(Value::as_str) != Some("message")
        || p.get("role").and_then(Value::as_str) != Some("user")
    {
        return;
    }
    let Some(content) = p.get("content").and_then(Value::as_array) else { return };
    let mut text = String::new();
    let mut imgs = Vec::new();
    let mut named: Option<u32> = None;
    let mut nth = 0;
    for b in content {
        match b.get("type").and_then(Value::as_str) {
            Some("input_text") => {
                let t = b.get("text").and_then(Value::as_str).unwrap_or("");
                if let Some(rest) = t.strip_prefix("<image name=") {
                    named = tags(rest).into_iter().next();
                } else if t != "</image>" {
                    text.push_str(t);
                }
            }
            Some("input_image") => {
                if let Some(n) = named.take() {
                    imgs.push((n, Loc { offset, len, nth }));
                }
                nth += 1;
            }
            _ => {}
        }
    }
    if !imgs.is_empty() {
        index.codex.push(Msg { text: squash(&text), imgs });
    }
}

/// The latest message with image `#n` whose text the row is part of; failing
/// that, the latest with `#n` at all.
fn pick_codex(msgs: &[Msg], n: u32, row: &str) -> Option<Loc> {
    let find = |m: &Msg| m.imgs.iter().find(|(k, _)| *k == n).map(|(_, l)| *l);
    let key = row_key(row);
    if !key.is_empty() {
        if let Some(l) = msgs.iter().rev().filter(|m| m.text.contains(&key)).find_map(find) {
            return Some(l);
        }
    }
    msgs.iter().rev().find_map(find)
}

/// Enough of a row to know its message by: the text after the prompt marker,
/// tags and spacing removed, as the message text is.
fn row_key(row: &str) -> String {
    let s = squash(row);
    let s = s.trim_start_matches(|c: char| !c.is_alphanumeric());
    let key: String = s.chars().take(16).collect();
    if key.chars().count() < 2 { String::new() } else { key }
}

/// Text with its `[Image #N]` tags and all whitespace taken out — the
/// terminal wraps and pads, the transcript does not.
fn squash(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("[Image #") {
        out.extend(rest[..i].chars().filter(|c| !c.is_whitespace()));
        let after = &rest[i + 8..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && after[digits..].starts_with(']') {
            rest = &after[digits + 1..];
        } else {
            out.push_str("[Image#");
            rest = after;
        }
    }
    out.extend(rest.chars().filter(|c| !c.is_whitespace()));
    out
}

/// Every `N` in `[Image #N]`, in order.
fn tags(s: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find("[Image #") {
        let after = &rest[i + 8..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && after[digits..].starts_with(']') {
            if let Ok(n) = after[..digits].parse() {
                out.push(n);
            }
        }
        rest = after;
    }
    out
}

/// Reads back the one line, and decodes the one image in it.
fn read_image(path: &Path, loc: Loc, agent: AgentKind) -> Option<Image> {
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(loc.offset)).ok()?;
    let mut buf = vec![0; loc.len];
    file.read_exact(&mut buf).ok()?;
    let v: Value = serde_json::from_slice(&buf).ok()?;
    let (mime, b64) = match agent {
        AgentKind::Claude => {
            let img = claude_blocks(&v)?.iter().filter(is_claude_image).nth(loc.nth)?;
            (
                img.pointer("/source/media_type")?.as_str()?.to_string(),
                img.pointer("/source/data")?.as_str()?,
            )
        }
        _ => {
            let url = v
                .pointer("/payload/content")?
                .as_array()?
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("input_image"))
                .nth(loc.nth)?
                .get("image_url")?
                .as_str()?;
            let (mime, b64) = url.strip_prefix("data:")?.split_once(";base64,")?;
            (mime.to_string(), b64)
        }
    };
    // Only ever handed to an <img>, but nothing else has any business here.
    if !mime.starts_with("image/") {
        return None;
    }
    let data = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    Some(Image { mime, data })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const PNG: &str = "iVBORw0KGgo="; // the PNG signature, which is all a test needs

    fn claude_line(text: &str, images: usize) -> String {
        let mut content = vec![serde_json::json!({"type": "text", "text": text})];
        for _ in 0..images {
            content.push(serde_json::json!({
                "type": "image",
                "source": {"type": "base64", "media_type": "image/png", "data": PNG}
            }));
        }
        serde_json::json!({"type": "user", "message": {"role": "user", "content": content}}).to_string()
    }

    fn codex_line(text: &str, nums: &[u32]) -> String {
        let mut content = Vec::new();
        for n in nums {
            content.push(serde_json::json!({"type": "input_text", "text": format!("<image name=[Image #{n}] path=\"/tmp/x.png\">")}));
            content.push(serde_json::json!({"type": "input_image", "image_url": format!("data:image/png;base64,{PNG}")}));
            content.push(serde_json::json!({"type": "input_text", "text": "</image>"}));
        }
        content.push(serde_json::json!({"type": "input_text", "text": text}));
        serde_json::json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": content}}).to_string()
    }

    fn indexed(agent: AgentKind, lines: &[String]) -> (tempfile_path::Tmp, Index) {
        let tmp = tempfile_path::Tmp::new(lines);
        let mut index = Index::default();
        update(&mut index, &tmp.0, agent).unwrap();
        (tmp, index)
    }

    /// A transcript on disk for one test, removed after.
    mod tempfile_path {
        use super::*;
        pub struct Tmp(pub PathBuf);
        impl Tmp {
            pub fn new(lines: &[String]) -> Self {
                static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let p = std::env::temp_dir().join(format!(
                    "beebox-images-{}-{}.jsonl",
                    std::process::id(),
                    N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                ));
                let mut f = File::create(&p).unwrap();
                for l in lines {
                    writeln!(f, "{l}").unwrap();
                }
                Tmp(p)
            }
        }
        impl Drop for Tmp {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
    }

    #[test]
    fn claude_maps_the_highest_tags_to_the_images() {
        let (tmp, index) = indexed(
            AgentKind::Claude,
            &[
                claude_line("[Image #1] first", 1),
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"ok"}]}}"#.into(),
                // Mentions #1 again; its own images are #7 and #8.
                claude_line("like [Image #1]: [Image #7] [Image #8]", 2),
            ],
        );
        assert_eq!(index.claude[&1].nth, 0);
        assert_eq!(index.claude[&7].nth, 0);
        assert_eq!(index.claude[&8].nth, 1);
        assert_ne!(index.claude[&1].offset, index.claude[&7].offset);
        let img = read_image(&tmp.0, index.claude[&8], AgentKind::Claude).unwrap();
        assert_eq!(img.mime, "image/png");
        assert!(img.data.starts_with(b"\x89PNG"));
    }

    #[test]
    fn a_half_written_line_waits_for_the_rest() {
        let tmp = tempfile_path::Tmp::new(&[claude_line("[Image #1] a", 1)]);
        let mut f = std::fs::OpenOptions::new().append(true).open(&tmp.0).unwrap();
        let next = claude_line("[Image #2] b", 1);
        f.write_all(next[..20].as_bytes()).unwrap();
        let mut index = Index::default();
        update(&mut index, &tmp.0, AgentKind::Claude).unwrap();
        assert!(index.claude.contains_key(&1) && !index.claude.contains_key(&2));
        writeln!(f, "{}", &next[20..]).unwrap();
        update(&mut index, &tmp.0, AgentKind::Claude).unwrap();
        assert!(index.claude.contains_key(&2));
    }

    #[test]
    fn codex_picks_the_message_by_its_row() {
        let (tmp, index) = indexed(
            AgentKind::Codex,
            &[
                codex_line("[Image #1] 第一条 消息", &[1]),
                codex_line("看这两张 [Image #1] [Image #2] 有什么区别", &[1, 2]),
                codex_line("[Image #1] 最后一条", &[1]),
            ],
        );
        let first = pick_codex(&index.codex, 1, "› [Image #1] 第一条 消息").unwrap();
        assert_eq!(first.offset, 0);
        let second = pick_codex(&index.codex, 2, "› 看这两张 [Image #1] [Image #2] 有什么").unwrap();
        assert_eq!(second.nth, 1);
        // No row to go by: the latest.
        let latest = pick_codex(&index.codex, 1, "  ⎿ [Image #1]").unwrap();
        assert_eq!(latest.offset, index.codex[2].imgs[0].1.offset);
        assert!(read_image(&tmp.0, second, AgentKind::Codex).is_some());
    }

    #[test]
    fn indexes_past_the_limit_go_least_recent_first() {
        let mut files = Files::default();
        for i in 0..=INDEXES {
            let p = PathBuf::from(format!("/t/{i}.jsonl"));
            files.indexes.insert(p.clone(), Index::default());
            files.touch(&p);
        }
        assert_eq!(files.indexes.len(), INDEXES);
        assert!(!files.indexes.contains_key(Path::new("/t/0.jsonl")));
        // Touching one again keeps it past the next eviction.
        files.touch(Path::new("/t/1.jsonl"));
        let p = PathBuf::from("/t/new.jsonl");
        files.indexes.insert(p.clone(), Index::default());
        files.touch(&p);
        assert!(files.indexes.contains_key(Path::new("/t/1.jsonl")));
        assert!(!files.indexes.contains_key(Path::new("/t/2.jsonl")));
    }

    #[test]
    fn squash_drops_tags_and_spacing_only() {
        assert_eq!(squash("› [Image #12]  这里 ok?"), "›这里ok?");
        assert_eq!(squash("[Image #x] y"), "[Image#x]y");
        assert_eq!(tags("[Image #3] and [Image #12], not [Image #]"), vec![3, 12]);
    }
}
