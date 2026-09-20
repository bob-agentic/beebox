//! Scrollback ring.
//!
//! Line-based and large by default: an agent working through a long task emits
//! tens of thousands of lines, and being unable to scroll back to what it did
//! an hour ago is a real failure of this product. So the default is generous.
//!
//! It stores raw bytes split on newlines — not a parsed grid — so the cost is
//! storage and nothing else.
//!
//! Every subscriber holds an absolute offset instead of only a channel. That is
//! what removes the seam where a live read loop would otherwise duplicate or
//! drop bytes while a slow client recovers.

use std::collections::VecDeque;

pub const DEFAULT_MAX_LINES: usize = 200_000;

/// An unterminated line is sealed at this length so eviction can reclaim it.
const MAX_OPEN_LINE: usize = 1 << 20;

/// One stored line, including its terminator. The last entry may be partial:
/// PTY reads split mid-line constantly.
#[derive(Debug)]
struct Line {
    bytes: Vec<u8>,
    /// Absolute offset of this line's first byte.
    start: u64,
    complete: bool,
}

#[derive(Debug)]
pub struct Ring {
    lines: VecDeque<Line>,
    max_lines: usize,
    /// Total bytes ever written. Offsets are absolute and monotonic, so they
    /// stay meaningful after eviction.
    written: u64,
    /// Offset of the oldest byte still held.
    oldest: u64,
}

impl Ring {
    pub fn new(max_lines: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            max_lines: max_lines.max(1),
            written: 0,
            oldest: 0,
        }
    }

    /// Total bytes ever written — the offset the next byte will land at.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Offset of the oldest retained byte. A subscriber below this has fallen
    /// off the end and needs a full resync.
    pub fn oldest(&self) -> u64 {
        self.oldest
    }

    pub fn len_bytes(&self) -> usize {
        self.lines.iter().map(|l| l.bytes.len()).sum()
    }

    pub fn len_lines(&self) -> usize {
        self.lines.len()
    }

    pub fn push(&mut self, chunk: &[u8]) {
        for &b in chunk {
            // Continue the open line, or start a new one.
            match self.lines.back_mut() {
                Some(last) if !last.complete => last.bytes.push(b),
                _ => self.lines.push_back(Line {
                    bytes: vec![b],
                    start: self.written,
                    complete: false,
                }),
            }
            self.written += 1;

            // Seal on a newline, or when an unterminated line grows past the
            // cap — a progress bar can redraw forever without ever emitting
            // one, and sealing is what lets eviction reclaim it.
            let seal = match self.lines.back() {
                Some(last) => b == b'\n' || last.bytes.len() >= MAX_OPEN_LINE,
                None => false,
            };
            if seal {
                if let Some(last) = self.lines.back_mut() {
                    last.complete = true;
                }
                self.evict();
            }
        }
    }

    fn evict(&mut self) {
        while self.lines.len() > self.max_lines {
            if let Some(dropped) = self.lines.pop_front() {
                self.oldest = dropped.start + dropped.bytes.len() as u64;
            }
        }
    }

    /// Bytes from `from` to the end. Offsets below [`Ring::oldest`] are clamped
    /// to what remains, so a caller that fell behind gets the most it can
    /// rather than an error.
    pub fn since(&self, from: u64) -> Vec<u8> {
        let mut out = Vec::new();
        for line in &self.lines {
            let end = line.start + line.bytes.len() as u64;
            if end <= from {
                continue;
            }
            if line.start >= from {
                out.extend_from_slice(&line.bytes);
            } else {
                // Partially consumed line: skip what the caller already has.
                let skip = (from - line.start) as usize;
                out.extend_from_slice(&line.bytes[skip..]);
            }
        }
        out
    }

    /// The tail, at most `lines` long. Used for the first frame a viewer sees:
    /// replaying 200k lines into a fresh terminal would be pointless.
    pub fn tail_lines(&self, lines: usize) -> (Vec<u8>, u64) {
        let skip = self.lines.len().saturating_sub(lines);
        let start = self
            .lines
            .get(skip)
            .map(|l| l.start)
            .unwrap_or(self.written);
        (self.since(start), start)
    }
}

impl Default for Ring {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_LINES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_are_absolute_and_survive_eviction() {
        let mut r = Ring::new(2);
        r.push(b"one\ntwo\nthree\n");

        assert_eq!(r.written(), 14);
        assert_eq!(r.len_lines(), 2, "kept the last two");
        assert_eq!(r.oldest(), 4, "'one\\n' was evicted");
        // Offsets stay meaningful after eviction — that is the whole point.
        assert_eq!(r.since(4), b"two\nthree\n");
    }

    #[test]
    fn since_resumes_mid_line() {
        // A subscriber's offset can land anywhere, including inside a line.
        let mut r = Ring::new(10);
        r.push(b"hello world\n");
        assert_eq!(r.since(6), b"world\n");
    }

    #[test]
    fn since_past_the_end_is_empty() {
        let mut r = Ring::new(10);
        r.push(b"abc\n");
        assert!(r.since(4).is_empty(), "caller is already current");
        assert!(r.since(999).is_empty(), "never panics on a stale offset");
    }

    #[test]
    fn a_stale_offset_yields_what_remains() {
        let mut r = Ring::new(2);
        r.push(b"a\nb\nc\nd\n");
        // Offset 0 is long gone; the caller gets the surviving tail, not an error.
        assert_eq!(r.since(0), b"c\nd\n");
    }

    #[test]
    fn partial_writes_join_the_open_line() {
        // The common case: PTY reads split mid-line.
        let mut r = Ring::new(10);
        r.push(b"par");
        r.push(b"tial");
        assert_eq!(r.len_lines(), 1);
        r.push(b"\n");
        assert_eq!(r.since(0), b"partial\n");
    }

    #[test]
    fn tail_lines_bounds_the_first_replay() {
        let mut r = Ring::new(1000);
        for i in 0..100 {
            r.push(format!("line {i}\n").as_bytes());
        }
        let (bytes, start) = r.tail_lines(3);
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(text, "line 97\nline 98\nline 99\n");
        assert_eq!(r.since(start).len(), text.len());
    }

    #[test]
    fn tail_lines_handles_asking_for_more_than_exists() {
        let mut r = Ring::new(10);
        r.push(b"only\n");
        let (bytes, start) = r.tail_lines(100);
        assert_eq!(bytes, b"only\n");
        assert_eq!(start, 0);
    }

    #[test]
    fn an_endless_line_cannot_grow_without_bound() {
        // A progress bar that never emits a newline must not eat memory: the
        // open line gets sealed at MAX_OPEN_LINE so eviction can reclaim it,
        // bounding the ring at max_lines × MAX_OPEN_LINE rather than the total
        // written.
        let mut r = Ring::new(4);
        for _ in 0..40 {
            r.push(&vec![b'x'; 400_000]); // 16 MB written, no newline ever
        }
        // Ceiling is max_lines sealed lines plus the one still open.
        let ceiling = (4 + 1) * MAX_OPEN_LINE;
        assert!(r.len_bytes() <= ceiling, "held {} bytes", r.len_bytes());
        assert!(r.len_lines() <= 5);
        assert_eq!(r.written(), 40 * 400_000, "offsets still count every byte");
    }

    #[test]
    fn default_scrollback_is_generous() {
        // Scrolling back an hour is a product requirement, not a nicety.
        assert_eq!(Ring::default().max_lines, DEFAULT_MAX_LINES);
    }
}
