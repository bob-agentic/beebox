// Boots a real daemon on a scratch state directory and hands back its URL.
//
// Unit tests cannot catch what these tests are for: whether clicking a real
// button in a real browser, against a real PTY, actually does the thing.

import { spawn, type ChildProcess } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('../..', import.meta.url));
const DAEMON = join(ROOT, 'core/target/release/beebox-core');

export interface Daemon {
  url: string;
  home: string;
  stop: () => void;
  /** Everything the daemon has logged, for diagnosing a failure. */
  log: () => string;
}

/** Starts a daemon on its own port and state directory, so tests never
    collide with each other or with a running app. `opts.home` reuses an
    existing state directory (for restart tests); `opts.keepHome` leaves the
    directory behind on stop so a follow-up daemon can adopt it. */
export async function startDaemon(
  port: number,
  opts: { home?: string; keepHome?: boolean } = {},
): Promise<Daemon> {
  const home = opts.home ?? mkdtempSync(join(tmpdir(), 'beebox-e2e-'));
  const proc: ChildProcess = spawn(
    DAEMON,
    [
      '--home', home,
      '--cwd', ROOT,
      '--ui', join(ROOT, 'ui/dist'),
      '--listen', `127.0.0.1:${port}`,
    ],
    { stdio: ['ignore', 'pipe', 'pipe'] },
  );

  let out = '';
  proc.stdout?.on('data', (d) => (out += d));
  proc.stderr?.on('data', (d) => (out += d));

  // The owner key is printed at startup; without it the socket is refused.
  const key = await new Promise<string>((resolve, reject) => {
    const deadline = setTimeout(
      () => reject(new Error(`daemon did not start:\n${out}`)),
      15_000,
    );
    const tick = setInterval(() => {
      const m = out.match(/key=([a-f0-9]{32})/);
      if (m) {
        clearInterval(tick);
        clearTimeout(deadline);
        resolve(m[1]);
      }
    }, 100);
  });

  return {
    url: `http://127.0.0.1:${port}/?key=${key}`,
    home,
    log: () => out,
    stop() {
      proc.kill('SIGTERM');
      if (opts.keepHome || opts.home) return;
      try {
        rmSync(home, { recursive: true, force: true });
      } catch {
        // Best effort; the temp dir is disposable.
      }
    },
  };
}
