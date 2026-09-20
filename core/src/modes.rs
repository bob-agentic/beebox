//! Mode sniffing.
//!
//! Not a VT parser. It watches the outbound byte stream for a fixed list of
//! `ESC[?<n>h|l` private-mode sequences and OSC title sets, and keeps a handful
//! of booleans. It interprets no content and buffers no grid.
//!
//! This exists because replaying a ring tail cuts the stream at an arbitrary
//! offset. Split UTF-8 and split escapes are cosmetic and self-healing; losing
//! `ESC[?1049h` (alt screen), `ESC[?2004h` (bracketed paste) or `ESC[?1h`
//! (application cursor keys) is not — the client would then send the *wrong
//! bytes* for arrow keys and pastes, in exactly the agent TUIs this product
//! exists to run.

/// Private modes worth tracking. Anything not listed is ignored.
const ALT_SCREEN: u16 = 1049;
const ALT_SCREEN_OLD: u16 = 47;
const BRACKETED_PASTE: u16 = 2004;
const APP_CURSOR_KEYS: u16 = 1;
const APP_KEYPAD: u16 = 66;
const MOUSE_X10: u16 = 9;
const MOUSE_VT200: u16 = 1000;
const MOUSE_BTN_EVENT: u16 = 1002;
const MOUSE_ANY_EVENT: u16 = 1003;
const MOUSE_SGR: u16 = 1006;
const FOCUS_EVENT: u16 = 1004;
const CURSOR_VISIBLE: u16 = 25;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modes {
    pub alt_screen: bool,
    pub bracketed_paste: bool,
    pub app_cursor_keys: bool,
    pub app_keypad: bool,
    pub mouse_x10: bool,
    pub mouse_vt200: bool,
    pub mouse_btn_event: bool,
    pub mouse_any_event: bool,
    pub mouse_sgr: bool,
    pub focus_event: bool,
    /// Cursor starts visible, so this one is inverted on the wire.
    pub cursor_hidden: bool,
}

impl Modes {
    /// The escape sequences that put a fresh terminal into this state. Sent as
    /// the `Resync` prefix, before the replayed tail.
    pub fn to_escapes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut set = |on: bool, n: u16| {
            if on {
                out.extend_from_slice(format!("\x1b[?{n}h").as_bytes());
            }
        };
        // Alt screen first: it resets much of the rest.
        set(self.alt_screen, ALT_SCREEN);
        set(self.bracketed_paste, BRACKETED_PASTE);
        set(self.app_cursor_keys, APP_CURSOR_KEYS);
        set(self.app_keypad, APP_KEYPAD);
        set(self.mouse_x10, MOUSE_X10);
        set(self.mouse_vt200, MOUSE_VT200);
        set(self.mouse_btn_event, MOUSE_BTN_EVENT);
        set(self.mouse_any_event, MOUSE_ANY_EVENT);
        set(self.mouse_sgr, MOUSE_SGR);
        set(self.focus_event, FOCUS_EVENT);
        if self.cursor_hidden {
            out.extend_from_slice(b"\x1b[?25l");
        }
        out
    }

    fn apply(&mut self, n: u16, on: bool) {
        match n {
            ALT_SCREEN | ALT_SCREEN_OLD => self.alt_screen = on,
            BRACKETED_PASTE => self.bracketed_paste = on,
            APP_CURSOR_KEYS => self.app_cursor_keys = on,
            APP_KEYPAD => self.app_keypad = on,
            MOUSE_X10 => self.mouse_x10 = on,
            MOUSE_VT200 => self.mouse_vt200 = on,
            MOUSE_BTN_EVENT => self.mouse_btn_event = on,
            MOUSE_ANY_EVENT => self.mouse_any_event = on,
            MOUSE_SGR => self.mouse_sgr = on,
            FOCUS_EVENT => self.focus_event = on,
            CURSOR_VISIBLE => self.cursor_hidden = !on,
            _ => {}
        }
    }
}

/// Where the sniffer is in a sequence that spans chunk boundaries. PTY reads
/// split anywhere, so the state has to survive between `feed` calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scan {
    Ground,
    /// Saw ESC.
    Esc,
    /// Inside `ESC[`. `private` is set by the single leading `?` and must hold
    /// for the whole parameter list.
    Csi { private: bool, param: u16, has_param: bool },
    /// Inside `ESC]` — collecting an OSC string.
    Osc,
    /// Saw ESC inside an OSC string; `\` terminates it (ST).
    OscEsc,
}

/// Feeds the outbound stream and tracks terminal state.
#[derive(Debug, Default)]
pub struct Sniffer {
    modes: Modes,
    scan: Scan,
    /// Parameters of the CSI sequence in flight.
    params: Vec<u16>,
    /// OSC 0/2 payload, for the pane title.
    osc: Vec<u8>,
    title: Option<String>,
    /// Absolute ring offset at which alt screen was most recently entered. An
    /// alt screen has no scrollback, so replaying earlier history into one is
    /// simply wrong — replay starts here instead.
    alt_entered_at: Option<u64>,
}

impl Default for Scan {
    fn default() -> Self {
        Scan::Ground
    }
}

/// Bounds so a malformed stream cannot grow memory.
const MAX_OSC: usize = 512;
const MAX_PARAMS: usize = 16;

impl Sniffer {
    pub fn modes(&self) -> Modes {
        self.modes
    }

    /// Title from the most recent OSC 0/2, consumed once so callers only emit
    /// `Out::Title` on change.
    pub fn take_title(&mut self) -> Option<String> {
        self.title.take()
    }

    /// Offset to start a replay from: the alt-screen entry point when in alt
    /// screen, otherwise `None` for "as much scrollback as you have".
    pub fn replay_from(&self) -> Option<u64> {
        self.modes.alt_screen.then_some(self.alt_entered_at).flatten()
    }

    /// `base` is the absolute ring offset of `chunk[0]`.
    pub fn feed(&mut self, chunk: &[u8], base: u64) {
        for (i, &b) in chunk.iter().enumerate() {
            let at = base + i as u64;
            self.scan = match (self.scan, b) {
                (Scan::Ground, 0x1b) => Scan::Esc,
                (Scan::Ground, _) => Scan::Ground,

                (Scan::Esc, b'[') => Scan::Csi { private: false, param: 0, has_param: false },
                (Scan::Esc, b']') => {
                    self.osc.clear();
                    Scan::Osc
                }
                (Scan::Esc, 0x1b) => Scan::Esc,
                (Scan::Esc, _) => Scan::Ground,

                (Scan::Csi { private, param, has_param }, c) => match c {
                    b'?' if !has_param && self.params.is_empty() => {
                        Scan::Csi { private: true, param, has_param }
                    }
                    b'0'..=b'9' => Scan::Csi {
                        private,
                        param: param.saturating_mul(10).saturating_add((c - b'0') as u16),
                        has_param: true,
                    },
                    // `ESC[?1000;1002;1006h` sets all three: collect now, apply
                    // when the final h/l says which way.
                    b';' => {
                        if has_param && self.params.len() < MAX_PARAMS {
                            self.params.push(param);
                        }
                        Scan::Csi { private, param: 0, has_param: false }
                    }
                    b'h' | b'l' => {
                        if has_param && self.params.len() < MAX_PARAMS {
                            self.params.push(param);
                        }
                        if private {
                            self.apply_params(c == b'h', at);
                        }
                        self.params.clear();
                        Scan::Ground
                    }
                    // Any other final byte ends the sequence; we don't care.
                    0x40..=0x7e => {
                        self.params.clear();
                        Scan::Ground
                    }
                    _ => Scan::Csi { private, param, has_param },
                },

                (Scan::Osc, 0x1b) => Scan::OscEsc,
                (Scan::Osc, 0x07) => {
                    self.finish_osc();
                    Scan::Ground
                }
                (Scan::Osc, c) => {
                    if self.osc.len() < MAX_OSC {
                        self.osc.push(c);
                    }
                    Scan::Osc
                }
                (Scan::OscEsc, b'\\') => {
                    self.finish_osc();
                    Scan::Ground
                }
                (Scan::OscEsc, _) => Scan::Osc,
            };
        }
    }

    /// `at` is the offset of the sequence's final byte, recorded so an
    /// alt-screen replay can start after the switch rather than before it.
    fn apply_params(&mut self, on: bool, at: u64) {
        for i in 0..self.params.len() {
            let n = self.params[i];
            if matches!(n, ALT_SCREEN | ALT_SCREEN_OLD) {
                self.alt_entered_at = on.then_some(at);
            }
            self.modes.apply(n, on);
        }
    }

    fn finish_osc(&mut self) {
        // OSC 0 (icon+title) and 2 (title) are the ones the pane header wants.
        let s = &self.osc;
        let rest = if let Some(r) = s.strip_prefix(b"0;") {
            r
        } else if let Some(r) = s.strip_prefix(b"2;") {
            r
        } else {
            self.osc.clear();
            return;
        };
        self.title = Some(String::from_utf8_lossy(rest).into_owned());
        self.osc.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sniff(bytes: &[u8]) -> Sniffer {
        let mut s = Sniffer::default();
        s.feed(bytes, 0);
        s
    }

    #[test]
    fn tracks_alt_screen_both_ways() {
        let mut s = sniff(b"\x1b[?1049h");
        assert!(s.modes().alt_screen);
        s.feed(b"\x1b[?1049l", 8);
        assert!(!s.modes().alt_screen);
    }

    #[test]
    fn tracks_the_modes_that_change_keystrokes() {
        // The reason this module exists: lose these and arrow keys and pastes
        // send the wrong bytes.
        let s = sniff(b"\x1b[?2004h\x1b[?1h");
        assert!(s.modes().bracketed_paste);
        assert!(s.modes().app_cursor_keys);
    }

    #[test]
    fn survives_a_sequence_split_across_chunks() {
        // PTY reads split anywhere; this is the common case, not an edge case.
        let mut s = Sniffer::default();
        s.feed(b"\x1b[?10", 0);
        s.feed(b"49h", 5);
        assert!(s.modes().alt_screen, "state must persist between feeds");
    }

    #[test]
    fn handles_multi_parameter_sequences() {
        let s = sniff(b"\x1b[?1000;1002;1006h");
        let m = s.modes();
        assert!(m.mouse_vt200 && m.mouse_btn_event && m.mouse_sgr);
    }

    #[test]
    fn escapes_replay_the_state_and_lead_with_alt_screen() {
        let s = sniff(b"\x1b[?1049h\x1b[?2004h\x1b[?1h\x1b[?25l");
        let esc = s.modes().to_escapes();
        let text = String::from_utf8(esc).unwrap();

        assert!(text.starts_with("\x1b[?1049h"), "alt screen resets much of the rest");
        assert!(text.contains("\x1b[?2004h"));
        assert!(text.contains("\x1b[?1h"));
        assert!(text.ends_with("\x1b[?25l"));
    }

    #[test]
    fn roundtrips_through_escapes() {
        let original = sniff(b"\x1b[?1049h\x1b[?2004h\x1b[?1006h\x1b[?25l");
        let replayed = sniff(&original.modes().to_escapes());
        assert_eq!(original.modes(), replayed.modes());
    }

    #[test]
    fn replay_starts_at_alt_screen_entry() {
        let mut s = Sniffer::default();
        s.feed(b"hello world", 0);
        s.feed(b"\x1b[?1049h", 11);
        // An alt screen has no scrollback; replaying the earlier text into one
        // would be wrong.
        assert_eq!(s.replay_from(), Some(18), "offset of the sequence's final byte");

        s.feed(b"\x1b[?1049l", 19);
        assert_eq!(s.replay_from(), None, "back to normal buffer: full scrollback");
    }

    #[test]
    fn reads_osc_titles() {
        let mut s = sniff(b"\x1b]0;npm run dev\x07");
        assert_eq!(s.take_title().as_deref(), Some("npm run dev"));
        assert!(s.take_title().is_none(), "consumed once");

        let mut s = sniff(b"\x1b]2;codex\x1b\\");
        assert_eq!(s.take_title().as_deref(), Some("codex"), "ST terminator");
    }

    #[test]
    fn ignores_unknown_modes_and_plain_output() {
        let s = sniff(b"regular text \x1b[1;32m colored \x1b[0m \x1b[?9999h");
        assert_eq!(s.modes(), Modes::default());
    }

    #[test]
    fn bounds_a_runaway_osc() {
        let mut s = Sniffer::default();
        s.feed(b"\x1b]0;", 0);
        s.feed(&vec![b'x'; 10_000], 4);
        assert!(s.osc.len() <= MAX_OSC, "unterminated OSC must not grow memory");
    }
}
