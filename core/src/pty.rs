//! PTY registry: spawn, read loop, coalescing, fan-out.
//!
//! One process per pane. The read loop feeds three things — the scrollback
//! ring, the mode sniffer, and every subscriber — and never blocks on any of
//! them. A slow phone must not stall the terminal or the other viewers.

use std::collections::HashMap;
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

/// How much scrollback a newly attached viewer receives when it does not ask
/// for a particular amount. Clients say what their own buffer holds; this is
/// the answer for one that does not.
pub const ATTACH_LINES: usize = 10_000;

/// Ceiling on what a client may ask to have replayed. It arrives from the
/// page, and serialising the whole ring for every pane on connect is not
/// something a query string gets to ask for.
pub const MAX_ATTACH_LINES: usize = 200_000;

/// What the read loop publishes. Subscribers translate this into `Out` frames.
#[derive(Debug, Clone)]
pub enum PtyEvent {
    Output { pane: PaneId, pty: PtyId, data: Arc<Vec<u8>> },
    Title { pane: PaneId, text: String },
    Exited { pane: PaneId, code: i32 },
}

/// Everything about one live process.
struct Pty {
    pane: PaneId,
    ring: Ring,
    sniffer: Sniffer,
    writer: Box<dyn Write + Send>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    cols: u16,
    rows: u16,
    /// The shell's pid, for cwd polling of its foreground process group.
    child_pid: Option<u32>,
}

pub struct Spawn {
    pub pane: PaneId,
    pub cmd: Vec<String>,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    /// Extra environment, used to point agent hooks at this pane.
    pub env: Vec<(String, String)>,
    pub scrollback_lines: usize,
}

pub struct Registry {
    ptys: Mutex<HashMap<PtyId, Pty>>,
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
        for (k, v) in std::env::vars() {
            cmd.env(k, v);
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
                ring: Ring::new(spec.scrollback_lines),
                sniffer: Sniffer::default(),
                writer,
                master: pair.master,
                cols: spec.cols,
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
            // Re-use the same channel for the exit signal: an empty vec.
            let _ = raw_tx.blocking_send(Vec::new());
            let _ = code_tx.send(code);
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
                        Some(c) if c.is_empty() => break,   // process exited
                        Some(c) => {
                            pending.extend_from_slice(&c);
                            if pending.len() >= FLUSH_BYTES {
                                reg.flush(id, pane, &mut pending);
                            }
                        }
                        None => break,
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
            reg.ptys.lock().unwrap().remove(&id);
            // The reader thread is the only one that can wait on the child, so
            // the status comes back from there. -1 if it never arrived: the
            // thread died without reaping, which is not a clean exit.
            let code = code_rx.await.unwrap_or(-1);
            let _ = reg.tx.send(PtyEvent::Exited { pane, code });
        });

        Ok(id)
    }

    /// Records a chunk in the ring, updates sniffed state, and fans it out.
    fn flush(&self, id: PtyId, pane: PaneId, pending: &mut Vec<u8>) {
        let title = {
            let mut ptys = self.ptys.lock().unwrap();
            let Some(p) = ptys.get_mut(&id) else {
                pending.clear();
                return;
            };
            let base = p.ring.written();
            p.sniffer.feed(pending, base);
            p.ring.push(pending);
            p.sniffer.take_title()
        };

        let data = Arc::new(std::mem::take(pending));
        let _ = self.tx.send(PtyEvent::Output { pane, pty: id, data });
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
        if p.cols == cols && p.rows == rows {
            return Ok(()); // avoid a pointless SIGWINCH repaint
        }
        p.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
        p.cols = cols;
        p.rows = rows;
        Ok(())
    }

    pub fn size(&self, id: PtyId) -> Option<(u16, u16)> {
        let ptys = self.ptys.lock().unwrap();
        ptys.get(&id).map(|p| (p.cols, p.rows))
    }

    /// Everything a newly attached viewer needs: the mode prefix that puts its
    /// terminal into the right state, then the scrollback tail.
    ///
    /// In alt screen the replay starts at the switch — an alt screen has no
    /// scrollback, so earlier history does not belong in it.
    pub fn attach_snapshot(
        &self,
        id: PtyId,
        lines: Option<usize>,
    ) -> Option<(Vec<u8>, Vec<u8>, u64)> {
        let lines = lines.unwrap_or(ATTACH_LINES).clamp(1, MAX_ATTACH_LINES);
        let ptys = self.ptys.lock().unwrap();
        let p = ptys.get(&id)?;
        let modes = p.sniffer.modes().to_escapes();

        let (data, _from) = match p.sniffer.replay_from() {
            Some(alt) => (p.ring.since(alt), alt),
            None => p.ring.tail_lines(lines),
        };
        Some((modes, data, p.ring.written()))
    }

    /// Replay for a subscriber that fell behind, from its own offset.
    pub fn resync_from(&self, id: PtyId, from: u64) -> Option<(Vec<u8>, Vec<u8>, u64)> {
        let ptys = self.ptys.lock().unwrap();
        let p = ptys.get(&id)?;
        let start = from.max(p.ring.oldest());
        Some((
            p.sniffer.modes().to_escapes(),
            p.ring.since(start),
            p.ring.written(),
        ))
    }

    pub fn modes(&self, id: PtyId) -> Option<Modes> {
        let ptys = self.ptys.lock().unwrap();
        ptys.get(&id).map(|p| p.sniffer.modes())
    }

    pub fn kill(&self, id: PtyId) {
        // Dropping the master closes the PTY, which ends the read loop, which
        // reaps the child.
        self.ptys.lock().unwrap().remove(&id);
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
            scrollback_lines: 1000,
        }
    }

    /// Collects output frames for one pane until the process exits.
    async fn drain(reg: &Arc<Registry>, id: PtyId, mut rx: broadcast::Receiver<PtyEvent>) -> String {
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
        let _ = reg;
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
        let id = reg
            .spawn(spec(1, &["sh", "-c", "echo hello beebox; sleep 0.2"]))
            .unwrap();
        assert!(drain(&reg, id, rx).await.contains("hello beebox"));
    }

    #[tokio::test]
    async fn panes_get_color_env_and_never_no_color() {
        // The desktop shell hands the daemon NO_COLOR=1 for its own log
        // parsing; a pane inheriting it turns Claude/Codex monochrome.
        std::env::set_var("NO_COLOR", "1");
        let reg = Registry::new();
        let rx = reg.subscribe();
        let id = reg
            .spawn(spec(1, &["sh", "-c", "echo NC=${NO_COLOR:-unset} CT=$COLORTERM; sleep 0.2"]))
            .unwrap();
        std::env::remove_var("NO_COLOR");
        let out = drain(&reg, id, rx).await;
        assert!(out.contains("NC=unset"), "NO_COLOR leaked into the pane: {out}");
        assert!(out.contains("CT=truecolor"), "COLORTERM missing: {out}");
    }

    #[tokio::test]
    async fn passes_utf8_through_untouched() {
        // Bytes-through is the core promise; CJK and emoji must survive.
        let reg = Registry::new();
        let rx = reg.subscribe();
        let id = reg
            .spawn(spec(1, &["sh", "-c", "echo '中文测试 ✓ 你好'; sleep 0.2"]))
            .unwrap();
        let out = drain(&reg, id, rx).await;
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

        assert!(drain(&reg, id, rx).await.contains("round trip"));
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
    async fn attach_replay_is_sized_to_what_the_client_asked_for() {
        // The client says what its own buffer holds. Sending more than that is
        // parsing work thrown away on arrival; sending less leaves it half
        // empty. A request past the ring's own size is simply all of it.
        let reg = Registry::new();
        let rx = reg.subscribe();
        // Kept alive past the output: attach_snapshot reads a live pty, and a
        // command that exits takes its registry entry with it.
        let id = reg
            .spawn(spec(
                1,
                &["sh", "-c", "for i in $(seq 1 200); do echo line $i; done; sleep 2"],
            ))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;

        let count = |lines: Option<usize>| {
            let (_, data, _) = reg.attach_snapshot(id, lines).expect("pty is live");
            String::from_utf8_lossy(&data).lines().count()
        };
        assert!(count(Some(10)) <= 10, "a small ask gets a small replay");
        assert!(count(Some(10)) < count(Some(150)), "a larger ask gets more");
        // Absurd asks are clamped rather than refused.
        assert!(count(Some(usize::MAX)) > 0);
        let _ = drain(&reg, id, rx).await;
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

        let (modes, data, through) = reg.attach_snapshot(id, None).expect("pty is live");
        let modes = String::from_utf8_lossy(&modes);
        assert!(modes.contains("\x1b[?1049h"), "alt screen must be restored");
        assert!(modes.contains("\x1b[?2004h"), "bracketed paste must be restored");
        assert!(through > 0);
        // In alt screen the replay starts at the switch, so it holds the
        // in-alt text and not what came before.
        assert!(String::from_utf8_lossy(&data).contains("in-alt"));
        let _ = drain(&reg, id, rx).await;
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
        let _ = drain(&reg, id, rx).await;
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
                Ok(Ok(PtyEvent::Exited { pane, code })) => {
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
                            if let Some((_, data, _)) = reg.attach_snapshot(*pty, None) {
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

