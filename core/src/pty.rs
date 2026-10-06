//! PTY registry: spawn, read loop, coalescing, fan-out.
//!
//! One process per pane. The read loop feeds three things — the scrollback
//! ring, the mode sniffer, and every subscriber — and never blocks on any of
//! them. A slow phone must not stall the terminal or the other viewers.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tokio::sync::{broadcast, mpsc};

use crate::modes::{Modes, Sniffer};
use crate::proto::{PaneId, PtyId};
use crate::ring::Ring;

/// Flush a pane's output every 4 ms or 64 KB, whichever comes first. Turns a
/// `yes` flood into ~250 frames/sec instead of one per read, while keeping any
/// single frame small enough that a subscriber's byte budget is meaningful.
const FLUSH_INTERVAL: Duration = Duration::from_millis(4);
const FLUSH_BYTES: usize = 64 * 1024;

/// What the read loop publishes. Subscribers translate this into `Out` frames.
#[derive(Debug, Clone)]
pub enum PtyEvent {
    /// `end` is the ring offset just past `data`. A snapshot's `through` is
    /// one of these, so a subscriber can tell what it already has.
    Output { pane: PaneId, pty: PtyId, data: Arc<Vec<u8>>, end: u64 },
    Title { pane: PaneId, text: String },
    Exited { pane: PaneId, pty: PtyId, code: i32 },
}

/// Our own OSC, `ESC ] 7788 ; cols BEL`: the width the output after it was
/// written at. It goes into the stream itself, live and in the ring, so a
/// client resizes its terminal exactly between the bytes drawn for the old
/// width and those drawn for the new — the order a real terminal sees them in.
fn width_marker(cols: u16) -> Vec<u8> {
    format!("\x1b]7788;{cols}\x07").into_bytes()
}

/// Everything about one live process.
struct Pty {
    pane: PaneId,
    hist: History,
    writer: Box<dyn Write + Send>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    rows: u16,
    /// The shell's pid, for cwd polling of its foreground process group.
    child_pid: Option<u32>,
}

/// A replay: the mode prefix, the output, and the ring offset it runs through.
pub type Snapshot = (Vec<u8>, Vec<u8>, u64);

/// What a process printed, and what a replay of it needs. Outlives the
/// process: a pane whose process failed stays open to be read.
struct History {
    ring: Ring,
    sniffer: Sniffer,
    cols: u16,
    /// The ring offset each width took effect at, oldest first. The ring
    /// carries a marker for every change; this is for where a replay starts
    /// between two of them.
    widths: VecDeque<(u64, u16)>,
}

impl History {
    /// Puts a width change into the ring, and returns the marker to send.
    fn set_width(&mut self, cols: u16) -> Vec<u8> {
        let marker = width_marker(cols);
        self.ring.push(&marker);
        self.cols = cols;
        self.widths.push_back((self.ring.written(), cols));
        // What the ring has dropped needs no width, except the one it still
        // starts in.
        let oldest = self.ring.oldest();
        while self.widths.get(1).is_some_and(|w| w.0 <= oldest) {
            self.widths.pop_front();
        }
        marker
    }

    /// The ring from `from` on, opening with the width it starts at.
    fn replay(&self, from: u64) -> Vec<u8> {
        let from = from.max(self.ring.oldest());
        let opening = self
            .widths
            .iter()
            .rev()
            .find(|w| w.0 <= from)
            .map_or(self.cols, |w| w.1);
        let mut out = width_marker(opening);
        out.extend(self.ring.since(from));
        out
    }

    /// The whole ring, which the viewer's own scrollback then trims: only a
    /// terminal knows how many lines the bytes make, since a program redrawing
    /// in place writes many newlines for each one it leaves. In alt screen the
    /// replay starts at the switch — an alt screen has no scrollback, so
    /// earlier history does not belong in it.
    fn snapshot(&self) -> Snapshot {
        let from = self.sniffer.replay_from().unwrap_or(self.ring.oldest());
        (self.sniffer.modes().to_escapes(), self.replay(from), self.ring.written())
    }
}

pub struct Spawn {
    pub pane: PaneId,
    pub cmd: Vec<String>,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    /// Extra environment, used to point agent hooks at this pane.
    pub env: Vec<(String, String)>,
    pub scrollback_bytes: usize,
}

pub struct Registry {
    ptys: Mutex<HashMap<PtyId, Pty>>,
    /// The history of each pane whose process ended on its own, with the pty
    /// it was — kept until the pane closes or re-runs (`keep_ended`).
    ended: Mutex<HashMap<PaneId, (PtyId, History)>>,
    /// Fan-out to every connected client. Bounded: on lag a subscriber resyncs
    /// from the ring rather than the channel.
    tx: broadcast::Sender<PtyEvent>,
    next_id: Mutex<u64>,
}

impl Registry {
    pub fn new() -> Arc<Self> {
        let (tx, _) = broadcast::channel(4096);
        Arc::new(Self {
            ptys: Mutex::new(HashMap::new()),
            ended: Mutex::new(HashMap::new()),
            tx,
            next_id: Mutex::new(0),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PtyEvent> {
        self.tx.subscribe()
    }

    /// Spawns a process and starts its read loop. Returns the new `PtyId`;
    /// the caller stores it on the pane.
    pub fn spawn(self: &Arc<Self>, spec: Spawn) -> Result<PtyId> {
        let sys = NativePtySystem::default();
        let pair = sys.openpty(PtySize {
            rows: spec.rows,
            cols: spec.cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(
            spec.cmd.first().ok_or_else(|| anyhow!("empty command"))?,
        );
        for a in &spec.cmd[1..] {
            cmd.arg(a);
        }
        cmd.cwd(&spec.cwd);
        // `CommandBuilder::new` starts from an empty environment, so everything
        // the shell needs has to be put back. Without HOME a prompt framework
        // cannot find its configuration and falls back to a bare `❯`; without
        // PATH almost nothing runs. This is invisible when the daemon is
        // started from a terminal — it inherits a full environment that way —
        // and only shows up when launched from a bundled app.
        //
        // Except Claude Code's own markers. A daemon started from inside a
        // Claude session carries them, and a Claude in a pane that inherits
        // them takes itself for a child session and writes no transcript —
        // leaving nothing for `--resume` to find. Removed explicitly, like
        // NO_COLOR below, since CommandBuilder inherits them anyway.
        for (k, v) in std::env::vars() {
            if k == "CLAUDECODE" || k.starts_with("CLAUDE_") {
                cmd.env_remove(k);
            } else {
                cmd.env(k, v);
            }
        }
        // The desktop shell launches this daemon with NO_COLOR=1 so the
        // startup log (which the shell parses for the key) is plain. That is
        // between the shell and the daemon — leaking it into every terminal
        // turns Claude/Codex monochrome. CommandBuilder inherits the parent
        // environment, so this must be an explicit remove, not a skip.
        cmd.env_remove("NO_COLOR");
        cmd.env("TERM", "xterm-256color");
        // Modern CLIs (Claude Code, Codex) key truecolor on COLORTERM, not
        // TERM. The daemon's own environment lacks it when launched from the
        // app bundle, and without it both agents render colourless. xterm.js
        // supports 24-bit colour, so advertising it is honest.
        cmd.env("COLORTERM", "truecolor");
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        let mut child = pair.slave.spawn_command(cmd)?;
        let child_pid = child.process_id();
        // The slave handle must be dropped or the PTY never reports EOF.
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        let id = {
            let mut n = self.next_id.lock().unwrap();
            *n += 1;
            *n
        };

        self.ptys.lock().unwrap().insert(
            id,
            Pty {
                pane: spec.pane,
                hist: History {
                    ring: Ring::new(spec.scrollback_bytes),
                    sniffer: Sniffer::default(),
                    cols: spec.cols,
                    widths: VecDeque::from([(0, spec.cols)]),
                },
                writer,
                master: pair.master,
                rows: spec.rows,
                child_pid,
            },
        );

        // Blocking reads belong on their own thread; the channel hands bytes to
        // the async side. The exit status travels separately: squeezing it into
        // the byte channel would mean inventing a sentinel that real output
        // could imitate.
        let (raw_tx, mut raw_rx) = mpsc::channel::<Vec<u8>>(64);
        let (code_tx, code_rx) = tokio::sync::oneshot::channel::<i32>();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = vec![0u8; 32 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if raw_tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
            let code = child
                .wait()
                .map(|s| s.exit_code() as i32)
                .unwrap_or(-1);
            let _ = code_tx.send(code);
            // Returning drops `raw_tx`, which is what ends the loop below.
        });

        // Coalescing loop.
        let reg = Arc::clone(self);
        let pane = spec.pane;
        tokio::spawn(async move {
            let mut pending: Vec<u8> = Vec::new();
            let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            loop {
                tokio::select! {
                    chunk = raw_rx.recv() => match chunk {
                        Some(c) => {
                            pending.extend_from_slice(&c);
                            if pending.len() >= FLUSH_BYTES {
                                reg.flush(id, pane, &mut pending);
                            }
                        }
                        None => break, // process exited
                    },
                    _ = ticker.tick() => {
                        if !pending.is_empty() {
                            reg.flush(id, pane, &mut pending);
                        }
                    }
                }
            }

            if !pending.is_empty() {
                reg.flush(id, pane, &mut pending);
            }
            // The history stays for as long as the pane does. Unless the pty
            // was killed, which is its pane closing.
            let gone = reg.ptys.lock().unwrap().remove(&id);
            if let Some(p) = gone {
                reg.ended.lock().unwrap().insert(pane, (id, p.hist));
            }
            // The reader thread is the only one that can wait on the child, so
            // the status comes back from there. -1 if it never arrived: the
            // thread died without reaping, which is not a clean exit.
            let code = code_rx.await.unwrap_or(-1);
            let _ = reg.tx.send(PtyEvent::Exited { pane, pty: id, code });
        });

        Ok(id)
    }

    /// Records a chunk in the ring, updates sniffed state, and fans it out.
    fn flush(&self, id: PtyId, pane: PaneId, pending: &mut Vec<u8>) {
        // A chunk goes into the ring whole, under the lock a snapshot takes
        // too, so every snapshot ends on a chunk boundary. And is sent under
        // it, so a width change pushed in between reaches clients in the
        // ring's order.
        let title = {
            let mut ptys = self.ptys.lock().unwrap();
            let Some(p) = ptys.get_mut(&id) else {
                pending.clear();
                return;
            };
            let h = &mut p.hist;
            let base = h.ring.written();
            h.sniffer.feed(pending, base);
            h.ring.push(pending);
            let end = h.ring.written();
            let data = Arc::new(std::mem::take(pending));
            let _ = self.tx.send(PtyEvent::Output { pane, pty: id, data, end });
            h.sniffer.take_title()
        };
        if let Some(text) = title {
            let _ = self.tx.send(PtyEvent::Title { pane, text });
        }
    }

    pub fn write(&self, id: PtyId, data: &[u8]) -> Result<()> {
        let mut ptys = self.ptys.lock().unwrap();
        let p = ptys.get_mut(&id).ok_or_else(|| anyhow!("no pty {id}"))?;
        p.writer.write_all(data)?;
        p.writer.flush()?;
        Ok(())
    }

    /// Applies a size. The caller has already decided it — clients only send
    /// advisory viewports, and the owner's wins.
    pub fn resize(&self, id: PtyId, cols: u16, rows: u16) -> Result<()> {
        let mut ptys = self.ptys.lock().unwrap();
        let p = ptys.get_mut(&id).ok_or_else(|| anyhow!("no pty {id}"))?;
        if p.hist.cols == cols && p.rows == rows {
            return Ok(()); // avoid a pointless SIGWINCH repaint
        }
        // Before the resize, so what the program redraws for the new width
        // comes after the marker.
        if p.hist.cols != cols {
            let marker = p.hist.set_width(cols);
            let end = p.hist.ring.written();
            let data = Arc::new(marker);
            let _ = self.tx.send(PtyEvent::Output { pane: p.pane, pty: id, data, end });
        }
        p.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
        p.rows = rows;
        Ok(())
    }

    /// Makes the program repaint its screen. A replay rebuilds what was
    /// printed, but a TUI caught mid-redraw, or drawn at a width since
    /// changed, only comes out right when the program draws it again. Setting
    /// the size it already has sends no SIGWINCH, so the height is nudged and
    /// put back — shpool does the same on attach.
    pub fn redraw(self: &Arc<Self>, id: PtyId) {
        if !self.nudge(id, 1) {
            return;
        }
        let reg = Arc::clone(self);
        tokio::spawn(async move {
            // Long enough for the program to have seen the first size.
            tokio::time::sleep(Duration::from_millis(50)).await;
            reg.nudge(id, 0);
        });
    }

    /// Sets the pty `less` rows short of its size, or back to it with 0.
    fn nudge(&self, id: PtyId, less: u16) -> bool {
        let ptys = self.ptys.lock().unwrap();
        let Some(p) = ptys.get(&id) else { return false };
        if p.rows <= less {
            return false;
        }
        let size = PtySize { rows: p.rows - less, cols: p.hist.cols, pixel_width: 0, pixel_height: 0 };
        p.master.resize(size).is_ok()
    }

    pub fn size(&self, id: PtyId) -> Option<(u16, u16)> {
        let ptys = self.ptys.lock().unwrap();
        ptys.get(&id).map(|p| (p.hist.cols, p.rows))
    }

    /// Everything a newly attached viewer needs: the mode prefix that puts its
    /// terminal into the right state, then the scrollback.
    pub fn attach_snapshot(&self, id: PtyId) -> Option<Snapshot> {
        let ptys = self.ptys.lock().unwrap();
        Some(ptys.get(&id)?.hist.snapshot())
    }

    /// What a viewer that has everything through `from` still lacks, and the
    /// offset that brings it to. `None` once the ring has dropped any of it.
    pub fn since(&self, id: PtyId, from: u64) -> Option<(Vec<u8>, u64)> {
        let ptys = self.ptys.lock().unwrap();
        let ring = &ptys.get(&id)?.hist.ring;
        (from >= ring.oldest()).then(|| (ring.since(from), ring.written()))
    }

    /// The same for a pane whose process has ended, with the pty it was.
    pub fn ended_snapshot(&self, pane: PaneId) -> Option<(PtyId, Snapshot)> {
        let ended = self.ended.lock().unwrap();
        let (pty, hist) = ended.get(&pane)?;
        Some((*pty, hist.snapshot()))
    }

    /// Drops the history of every ended pane `keep` says no to, given the
    /// pane and the pty it was — one that closed, or runs again.
    pub fn keep_ended(&self, keep: impl Fn(PaneId, PtyId) -> bool) {
        self.ended.lock().unwrap().retain(|&pane, (pty, _)| keep(pane, *pty));
    }

    pub fn modes(&self, id: PtyId) -> Option<Modes> {
        let ptys = self.ptys.lock().unwrap();
        ptys.get(&id).map(|p| p.hist.sniffer.modes())
    }

    /// Ends a pty's processes, as closing a terminal window does: a hangup
    /// to whatever is in the foreground — an agent and the MCP servers it
    /// started share its process group — and to the shell. Dropping the
    /// master alone never did it: the reader thread holds a clone, so the
    /// terminal never hung up, and every closed tab left its shell and agent
    /// running. Their exit ends the read loop, which reaps the child.
    pub fn kill(&self, id: PtyId) {
        let Some(p) = self.ptys.lock().unwrap().remove(&id) else { return };
        // SAFETY: plain signal sends to pids this pty started.
        unsafe {
            if let Some(fg) = p.master.process_group_leader() {
                libc::killpg(fg, libc::SIGHUP);
            }
            if let Some(pid) = p.child_pid {
                libc::kill(pid as libc::pid_t, libc::SIGHUP);
            }
        }
    }

    pub fn pane_of(&self, id: PtyId) -> Option<PaneId> {
        let ptys = self.ptys.lock().unwrap();
        ptys.get(&id).map(|p| p.pane)
    }

    /// The shell's pid, for cwd polling. `None` if the pty is gone.
    pub fn child_pid(&self, id: PtyId) -> Option<u32> {
        let ptys = self.ptys.lock().unwrap();
        ptys.get(&id).and_then(|p| p.child_pid)
    }

    /// Every live (pane, pid) pair, for the cwd poller's sweep.
    pub fn live_pids(&self) -> Vec<(PaneId, u32)> {
        let ptys = self.ptys.lock().unwrap();
        ptys.values()
            .filter_map(|p| p.child_pid.map(|pid| (p.pane, pid)))
            .collect()
    }

    pub fn live_count(&self) -> usize {
        self.ptys.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(pane: PaneId, cmd: &[&str]) -> Spawn {
        Spawn {
            pane,
            cmd: cmd.iter().map(|s| s.to_string()).collect(),
            cwd: "/tmp".into(),
            cols: 80,
            rows: 24,
            env: Vec::new(),
            scrollback_bytes: 1 << 20,
        }
    }

    /// Collects output frames for one pane until the process exits.
    async fn drain(mut rx: broadcast::Receiver<PtyEvent>) -> String {
        let mut out = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline - tokio::time::Instant::now();
            match tokio::time::timeout(left, rx.recv()).await {
                Ok(Ok(PtyEvent::Output { data, .. })) => out.extend_from_slice(&data),
                Ok(Ok(PtyEvent::Exited { .. })) => break,
                Ok(Ok(_)) => {}
                Ok(Err(_)) | Err(_) => break,
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    async fn spawns_and_captures_output() {
        let reg = Registry::new();
        let rx = reg.subscribe();
        // The trailing sleep is load-bearing. A process that exits in the same
        // breath as its write races the reader thread, which sees EOF and stops
        // before the bytes land — output lost, test failed, perhaps one run in
        // four under a loaded `cargo test`. Real panes run `zsh -l -i`, which
        // never exits, so the race is the test's alone. `modes_are_sniffed`
        // already handled it this way; the rest hadn't caught up.
        reg
            .spawn(spec(1, &["sh", "-c", "echo hello beebox; sleep 0.2"]))
            .unwrap();
        assert!(drain(rx).await.contains("hello beebox"));
    }

    #[tokio::test]
    async fn panes_do_not_inherit_claude_session_markers() {
        std::env::set_var("CLAUDECODE", "1");
        std::env::set_var("CLAUDE_CODE_CHILD_SESSION", "1");
        let reg = Registry::new();
        let rx = reg.subscribe();
        reg
            .spawn(spec(1, &["sh", "-c", "echo CC=${CLAUDECODE:-unset} CS=${CLAUDE_CODE_CHILD_SESSION:-unset}; sleep 0.2"]))
            .unwrap();
        std::env::remove_var("CLAUDECODE");
        std::env::remove_var("CLAUDE_CODE_CHILD_SESSION");
        let out = drain(rx).await;
        assert!(out.contains("CC=unset CS=unset"), "Claude markers leaked into the pane: {out}");
    }

    #[tokio::test]
    async fn panes_get_color_env_and_never_no_color() {
        // The desktop shell hands the daemon NO_COLOR=1 for its own log
        // parsing; a pane inheriting it turns Claude/Codex monochrome.
        std::env::set_var("NO_COLOR", "1");
        let reg = Registry::new();
        let rx = reg.subscribe();
        reg
            .spawn(spec(1, &["sh", "-c", "echo NC=${NO_COLOR:-unset} CT=$COLORTERM; sleep 0.2"]))
            .unwrap();
        std::env::remove_var("NO_COLOR");
        let out = drain(rx).await;
        assert!(out.contains("NC=unset"), "NO_COLOR leaked into the pane: {out}");
        assert!(out.contains("CT=truecolor"), "COLORTERM missing: {out}");
    }

    #[tokio::test]
    async fn passes_utf8_through_untouched() {
        // Bytes-through is the core promise; CJK and emoji must survive.
        let reg = Registry::new();
        let rx = reg.subscribe();
        reg
            .spawn(spec(1, &["sh", "-c", "echo '中文测试 ✓ 你好'; sleep 0.2"]))
            .unwrap();
        let out = drain(rx).await;
        assert!(out.contains("中文测试 ✓ 你好"), "got {out:?}");
    }

    #[tokio::test]
    async fn input_reaches_the_process() {
        let reg = Registry::new();
        let rx = reg.subscribe();
        let id = reg.spawn(spec(1, &["cat"])).unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;
        reg.write(id, b"round trip\n").unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        reg.kill(id);

        assert!(drain(rx).await.contains("round trip"));
    }

    #[tokio::test]
    async fn coalescing_beats_one_frame_per_read() {
        // 20k lines would be thousands of reads; batching must collapse them.
        let reg = Registry::new();
        let mut rx = reg.subscribe();
        reg.spawn(spec(1, &["sh", "-c", "seq 1 20000"])).unwrap();

        let mut frames = 0usize;
        let mut bytes = 0usize;
        loop {
            match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
                Ok(Ok(PtyEvent::Output { data, .. })) => {
                    frames += 1;
                    bytes += data.len();
                }
                Ok(Ok(PtyEvent::Exited { .. })) | Ok(Err(_)) | Err(_) => break,
                Ok(Ok(_)) => {}
            }
        }
        assert!(bytes > 100_000, "only {bytes} bytes");
        assert!(frames < 200, "{frames} frames for {bytes} bytes — not coalescing");
    }

    #[tokio::test]
    async fn attach_snapshot_carries_modes_then_scrollback() {
        let reg = Registry::new();
        let rx = reg.subscribe();
        // Enter alt screen, then print inside it.
        let id = reg
            .spawn(spec(1, &["sh", "-c", "printf '\\033[?1049h\\033[?2004hin-alt\\n'; sleep 0.4"]))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;

        let (modes, data, through) = reg.attach_snapshot(id).expect("pty is live");
        let modes = String::from_utf8_lossy(&modes);
        assert!(modes.contains("\x1b[?1049h"), "alt screen must be restored");
        assert!(modes.contains("\x1b[?2004h"), "bracketed paste must be restored");
        assert!(through > 0);
        // In alt screen the replay starts at the switch, so it holds the
        // in-alt text and not what came before.
        assert!(String::from_utf8_lossy(&data).contains("in-alt"));
        let _ = drain(rx).await;
    }

    #[tokio::test]
    async fn output_says_where_it_ends_in_the_ring() {
        // What lets a subscriber drop the output a snapshot already holds:
        // each chunk's end is an offset the snapshot's `through` can match.
        let reg = Registry::new();
        let mut rx = reg.subscribe();
        let id = reg
            .spawn(spec(1, &["sh", "-c", "printf one; sleep 0.2; printf two; sleep 0.4"]))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        let (_, _, through) = reg.attach_snapshot(id).unwrap();

        let mut ends = Vec::new();
        let mut total = 0;
        while let Ok(PtyEvent::Output { data, end, .. }) = rx.try_recv() {
            total += data.len() as u64;
            assert_eq!(end, total, "each end is the running total");
            ends.push(end);
        }
        assert_eq!(ends.last(), Some(&through), "the snapshot ends where the last chunk did");
        let _ = drain(rx).await;
    }

    #[tokio::test]
    async fn resize_is_idempotent_and_reported() {
        let reg = Registry::new();
        let rx = reg.subscribe();
        let id = reg.spawn(spec(1, &["sleep", "0.5"])).unwrap();

        assert_eq!(reg.size(id), Some((80, 24)));
        reg.resize(id, 96, 38).unwrap();
        assert_eq!(reg.size(id), Some((96, 38)));
        // A repeat must not fire another SIGWINCH.
        reg.resize(id, 96, 38).unwrap();
        assert_eq!(reg.size(id), Some((96, 38)));
        let _ = drain(rx).await;
    }

    #[tokio::test]
    async fn a_replay_marks_the_width_each_stretch_was_written_at() {
        let reg = Registry::new();
        let rx = reg.subscribe();
        let id = reg
            .spawn(spec(1, &["sh", "-c", "printf wide; sleep 0.4; printf narrow; sleep 0.6"]))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        reg.resize(id, 40, 24).unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;

        let (_, data, _) = reg.attach_snapshot(id).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&data),
            "\x1b]7788;80\x07wide\x1b]7788;40\x07narrow"
        );
        // Started after the change, it opens at the new width.
        {
            let ptys = reg.ptys.lock().unwrap();
            let h = &ptys[&id].hist;
            let from = h.ring.written() - "narrow".len() as u64;
            assert_eq!(String::from_utf8_lossy(&h.replay(from)), "\x1b]7788;40\x07narrow");
        }
        // Live, the change sits between the same bytes.
        assert_eq!(drain(rx).await, "wide\x1b]7788;40\x07narrow");
    }

    #[tokio::test]
    async fn redraw_signals_without_changing_the_size() {
        let reg = Registry::new();
        let rx = reg.subscribe();
        let id = reg
            .spawn(spec(1, &["sh", "-c", "trap 'printf winch' WINCH; sleep 0.3; sleep 0.3; sleep 0.3"]))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        reg.redraw(id);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(reg.size(id), Some((80, 24)));
        assert!(drain(rx).await.contains("winch"), "the program must hear of it");
    }

    #[tokio::test]
    async fn a_failing_command_reports_its_status() {
        // The code is what decides whether a pane closes on its own, so it has
        // to be the child's and not a placeholder.
        let reg = Registry::new();
        let mut rx = reg.subscribe();
        reg.spawn(spec(9, &["sh", "-c", "exit 3"])).unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Ok(PtyEvent::Exited { pane, code, .. })) => {
                    assert_eq!(pane, 9);
                    assert_eq!(code, 3, "the child's own status, not a stand-in");
                    break;
                }
                Ok(Ok(_)) => {}
                Ok(Err(_)) | Err(_) => panic!("never saw Exited"),
            }
        }
    }

    #[tokio::test]
    async fn exit_is_announced_and_the_pty_is_reaped() {
        let reg = Registry::new();
        let mut rx = reg.subscribe();
        reg.spawn(spec(7, &["true"])).unwrap();

        loop {
            match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
                Ok(Ok(PtyEvent::Exited { pane, .. })) => {
                    assert_eq!(pane, 7);
                    break;
                }
                Ok(Ok(_)) => {}
                Ok(Err(_)) | Err(_) => panic!("never saw Exited"),
            }
        }
        assert_eq!(reg.live_count(), 0, "registry must not leak dead ptys");
    }

    #[tokio::test]
    async fn killing_a_pty_ends_what_runs_in_it() {
        // Closing a tab, or freezing one, must not leave its agent running.
        let reg = Registry::new();
        let mut rx = reg.subscribe();
        let id = reg.spawn(spec(3, &["sh", "-c", "sleep 600 & echo pid=$!; wait"])).unwrap();
        let mut out = String::new();
        let child: libc::pid_t = loop {
            match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
                Ok(Ok(PtyEvent::Output { data, .. })) => {
                    out.push_str(&String::from_utf8_lossy(&data));
                    if let Some(n) = out.split("pid=").nth(1).and_then(|r| r.split_whitespace().next()) {
                        break n.parse().unwrap();
                    }
                }
                Ok(Ok(_)) => {}
                Ok(Err(_)) | Err(_) => panic!("never saw the child's pid"),
            }
        };
        reg.kill(id);
        loop {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Ok(PtyEvent::Exited { .. })) => break,
                Ok(Ok(_)) => {}
                Ok(Err(_)) | Err(_) => panic!("the shell never exited"),
            }
        }
        // SAFETY: signal 0 only asks whether the pid exists.
        assert_ne!(unsafe { libc::kill(child, 0) }, 0, "its child must be gone too");
    }

    #[tokio::test]
    async fn a_lagging_viewer_is_sent_only_what_it_missed() {
        let reg = Registry::new();
        let id = reg.spawn(spec(1, &["sh", "-c", "echo one; sleep 0.3; echo two; sleep 2"])).unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        let (_, _, sent) = reg.attach_snapshot(id).unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;

        let (missed, end) = reg.since(id, sent).expect("still in the ring");
        assert_eq!(String::from_utf8_lossy(&missed), "two\r\n");
        assert_eq!(end, reg.attach_snapshot(id).unwrap().2);
        assert!(reg.since(id, end).unwrap().0.is_empty(), "caught up");
    }

    #[tokio::test]
    async fn a_viewer_behind_what_the_ring_holds_needs_a_whole_replay() {
        let reg = Registry::new();
        let mut s = spec(1, &["sh", "-c", "for i in $(seq 1 200); do echo line $i; done; sleep 2"]);
        s.scrollback_bytes = 64;
        let id = reg.spawn(s).unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(reg.since(id, 0).is_none(), "the start was dropped");
    }

    #[tokio::test]
    async fn an_ended_process_can_still_be_replayed_until_let_go() {
        let reg = Registry::new();
        let mut rx = reg.subscribe();
        let id = reg
            .spawn(spec(7, &["sh", "-c", "echo last-words; sleep 0.2; exit 3"]))
            .unwrap();
        loop {
            match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
                Ok(Ok(PtyEvent::Exited { .. })) => break,
                Ok(Ok(_)) => {}
                Ok(Err(_)) | Err(_) => panic!("never saw Exited"),
            }
        }
        assert!(reg.attach_snapshot(id).is_none(), "the process is gone");

        let (pty, (_, data, _)) = reg.ended_snapshot(7).expect("history kept");
        assert_eq!(pty, id);
        assert!(String::from_utf8_lossy(&data).contains("last-words"));

        reg.keep_ended(|_, _| true);
        assert!(reg.ended_snapshot(7).is_some());
        reg.keep_ended(|pane, _| pane != 7);
        assert!(reg.ended_snapshot(7).is_none());
    }

    #[tokio::test]
    async fn a_killed_process_leaves_no_history() {
        let reg = Registry::new();
        let mut rx = reg.subscribe();
        let id = reg.spawn(spec(7, &["sleep", "5"])).unwrap();
        reg.kill(id);
        loop {
            match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
                Ok(Ok(PtyEvent::Exited { .. })) => break,
                Ok(Ok(_)) => {}
                Ok(Err(_)) | Err(_) => panic!("never saw Exited"),
            }
        }
        assert!(reg.ended_snapshot(7).is_none(), "its pane closed");
    }

    #[tokio::test]
    async fn several_panes_run_independently() {
        let reg = Registry::new();
        let mut rx = reg.subscribe();
        let mut live: HashMap<PaneId, PtyId> = HashMap::new();
        for pane in 1..=3u64 {
            // Same race as `spawns_and_captures_output`: hold the process open
            // past its write so the reader thread cannot miss the bytes.
            let id = reg
                .spawn(spec(pane, &["sh", "-c", &format!("echo pane{pane}; sleep 0.2")]))
                .unwrap();
            live.insert(pane, id);
        }

        // Wait for each pane's own output rather than for three exits: panes
        // finish independently, so an exit count can trip before a slower
        // pane has been heard from.
        let mut seen: HashMap<PaneId, String> = HashMap::new();
        let all_heard = |seen: &HashMap<PaneId, String>| {
            (1..=3u64).all(|p| {
                seen.get(&p).is_some_and(|s| s.contains(&format!("pane{p}")))
            })
        };
        while !all_heard(&seen) {
            match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
                Ok(Ok(PtyEvent::Output { pane, data, .. })) => {
                    seen.entry(pane)
                        .or_default()
                        .push_str(&String::from_utf8_lossy(&data));
                }
                Ok(Ok(_)) => {}
                // Three shells writing at once can outrun this loop and push a
                // frame out of the broadcast buffer. A real client answers that
                // by re-reading the ring, which is the source of truth — see
                // the `Lagged` arm in http.rs. Carrying on without doing the
                // same is what made this test lose a pane's only frame and
                // fail perhaps one run in three.
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                    for p in 1..=3u64 {
                        if let Some(pty) = live.get(&p) {
                            if let Some((_, data, _)) = reg.attach_snapshot(*pty) {
                                seen.entry(p)
                                    .or_default()
                                    .push_str(&String::from_utf8_lossy(&data));
                            }
                        }
                    }
                }
                Ok(Err(_)) | Err(_) => break,
            }
        }
        // Frames are never batched across panes, so each pane's bytes stay its own.
        for pane in 1..=3u64 {
            assert!(
                seen.get(&pane).is_some_and(|s| s.contains(&format!("pane{pane}"))),
                "pane {pane} output missing or mixed: {seen:?}"
            );
        }
    }
}

