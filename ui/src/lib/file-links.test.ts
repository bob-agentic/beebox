import { describe, expect, it } from 'vitest';
import { findPaths } from './file-links';

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

  it('leaves out words, numbers and URLs', () => {
    expect(paths('done in 0.2.0 — 93% of https://x.dev/a.js')).toEqual([]);
  });
});
