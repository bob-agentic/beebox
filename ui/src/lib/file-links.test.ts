import { describe, expect, it } from 'vitest';
import { findPaths, ResolveCache, RESOLVE_TTL_MS } from './file-links';

const paths = (s: string) => findPaths(s).map((m) => [m.path, m.line, m.col]);

describe('findPaths', () => {
  it('finds paths the way agents print them', () => {
    expect(paths('⏺ Update(ui/src/lib/components/Pane.svelte)')).toEqual([
      ['ui/src/lib/components/Pane.svelte', undefined, undefined],
    ]);
    expect(paths('see core/src/pty.rs:177:5, then main.py.')).toEqual([
      ['core/src/pty.rs', 177, 5],
      ['main.py', undefined, undefined],
    ]);
    expect(paths('"~/x/app-release.apk" and ./run.sh')).toEqual([
      ['~/x/app-release.apk', undefined, undefined],
      ['./run.sh', undefined, undefined],
    ]);
  });

  it('stops at CJK punctuation, which follows a path with no space', () => {
    expect(paths('.env 本身不进 ZIP，但 build.js 会编译进 dist/background.js。我检查（src/中文.md）')).toEqual([
      ['.env', undefined, undefined],
      ['build.js', undefined, undefined],
      ['dist/background.js', undefined, undefined],
      ['src/中文.md', undefined, undefined],
    ]);
  });

  it('covers exactly the path and its position', () => {
    const text = '  at a/b.ts:12: oops';
    const [m] = findPaths(text);
    expect(text.slice(m.start, m.end)).toBe('a/b.ts:12');
  });

  it('opens a range at its first line', () => {
    const text = 'app/core/event_bus.py:87-94 直接拿';
    const [m] = findPaths(text);
    expect([m.path, m.line, text.slice(m.start, m.end)]).toEqual([
      'app/core/event_bus.py',
      87,
      'app/core/event_bus.py:87',
    ]);
    expect(paths('a/build-2.js b/v1-2')).toEqual([
      ['a/build-2.js', undefined, undefined],
      ['b/v1-2', undefined, undefined],
    ]);
  });

  it('takes the path after an assignment', () => {
    const text = 'cd x && DOC=/a/docs/ARCH.md go test --file=b/c.ts';
    expect(paths(text)).toEqual([
      ['/a/docs/ARCH.md', undefined, undefined],
      ['b/c.ts', undefined, undefined],
    ]);
    const [m] = findPaths(text);
    expect(text.slice(m.start, m.end)).toBe('/a/docs/ARCH.md');
  });

  it('leaves out words, numbers and URLs', () => {
    expect(paths('done in 0.2.0 — 93% of https://x.dev/a.js')).toEqual([]);
  });
});

describe('ResolveCache', () => {
  function setup() {
    let t = 0;
    const asked: string[][] = [];
    const cache = new ResolveCache(
      async ({ paths }) => {
        asked.push(paths);
        return paths.map((p) => (p === 'gone.ts' ? null : `/real/${p}`));
      },
      () => t,
    );
    return { cache, asked, tick: (ms: number) => (t += ms) };
  }

  it('asks once, then answers on the spot', async () => {
    const { cache, asked } = setup();
    expect(cache.peek('/w', ['a.ts', 'gone.ts'])).toBeNull();
    expect(await cache.get('/w', ['a.ts', 'gone.ts'])).toEqual(['/real/a.ts', null]);
    // "Does not exist" is remembered as well.
    expect(cache.peek('/w', ['a.ts', 'gone.ts'])).toEqual(['/real/a.ts', null]);
    expect(asked).toEqual([['a.ts', 'gone.ts']]);
  });

  it('only asks about what it does not know, per folder', async () => {
    const { cache, asked } = setup();
    await cache.get('/w', ['a.ts']);
    await cache.get('/w', ['a.ts', 'b.ts']);
    await cache.get('/other', ['a.ts']);
    expect(asked).toEqual([['a.ts'], ['b.ts'], ['a.ts']]);
  });

  it('answers a stale entry at once and refreshes it behind', async () => {
    const { cache, asked, tick } = setup();
    await cache.get('/w', ['a.ts']);
    tick(RESOLVE_TTL_MS);
    expect(cache.peek('/w', ['a.ts'])).toEqual(['/real/a.ts']);
    expect(cache.peek('/w', ['a.ts'])).toEqual(['/real/a.ts']);
    await Promise.resolve();
    expect(asked).toEqual([['a.ts'], ['a.ts']]);
  });
});

describe('ResolveCache, asked twice at once', () => {
  it('shares one question between askers', async () => {
    let answer!: (v: (string | null)[]) => void;
    let asked = 0;
    const cache = new ResolveCache(() => {
      asked++;
      return new Promise((r) => (answer = r));
    });
    const one = cache.get('/w', ['a.ts']);
    const two = cache.get('/w', ['a.ts', 'a.ts']);
    answer(['/real/a.ts']);
    expect(await one).toEqual(['/real/a.ts']);
    expect(await two).toEqual(['/real/a.ts', '/real/a.ts']);
    expect(asked).toBe(1);
  });

  it('keeps the old answer when a refresh fails', async () => {
    let t = 0;
    let fail = false;
    const cache = new ResolveCache(
      async ({ paths }) => {
        if (fail) throw new Error('shell gone');
        return paths.map((p) => `/real/${p}`);
      },
      () => t,
    );
    await cache.get('/w', ['a.ts']);
    fail = true;
    t += RESOLVE_TTL_MS;
    expect(cache.peek('/w', ['a.ts'])).toEqual(['/real/a.ts']);
    await new Promise((r) => setTimeout(r, 0));
    expect(cache.peek('/w', ['a.ts'])).toEqual(['/real/a.ts']);
  });
});
