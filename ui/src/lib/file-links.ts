import type { ILink, ILinkProvider, Terminal } from '@xterm/xterm';

/** A file path printed in a terminal line, with the position an editor
 *  should jump to. `start`/`end` index the line's text, end exclusive. */
export interface PathMatch {
  start: number;
  end: number;
  path: string;
  line?: number;
  col?: number;
}

/** Runs of characters that can belong to a path. The delimiters are what
 *  agents wrap paths in: `Read(src/a.ts)`, `"a.py"`, `[x.md]`, box borders —
 *  and CJK punctuation, which a Chinese sentence puts straight after a path
 *  with no space: `dist/app.js。`. Han characters themselves stay, since a
 *  file may be named with them. */
const TOKEN = /[^\s'"`()[\]{}<>,;|│\u3000-\u303f\uff00-\uffef]+/g;
/** `path`, `path:12`, `path:12:3`, with sentence punctuation left behind. */
const PARTS = /^((.+?)(?::(\d+)(?::(\d+))?)?)[.:]*$/;
/** A bare name only counts with an extension, or as a dotfile: `Pane.svelte`
 *  and `.env`, not `done`. */
const BARE = /^(?:[\w@+-][\w.@+-]*\.[A-Za-z]\w*|\.[A-Za-z][\w.-]*)$/;

/** Everything in `text` that looks like a path. Only a guess — whether it
 *  names a real file is for whoever can see the disk to decide. */
export function findPaths(text: string): PathMatch[] {
  const out: PathMatch[] = [];
  for (const m of text.matchAll(TOKEN)) {
    const token = m[0];
    if (token.includes('://')) continue;
    const p = PARTS.exec(token);
    if (!p) continue;
    const [, whole, path, line, col] = p;
    if (!/\w/.test(path) || !(path.includes('/') || BARE.test(path))) continue;
    out.push({
      start: m.index,
      end: m.index + whole.length,
      path,
      line: line ? +line : undefined,
      col: col ? +col : undefined,
    });
  }
  return out;
}

/** The desktop shell's side: which paths exist, and opening one. */
interface Opener {
  resolvePaths(o: { paths: string[]; cwd: string }): Promise<(string | null)[]>;
  openFile(o: { path: string; line?: number; col?: number }): Promise<void>;
  /** The file under the mouse, for the right-click menu to offer. */
  hoverFile(o: { path: string | null }): Promise<void>;
}

/** Links over the file paths in a row, for `registerLinkProvider`. A path
 *  wrapped onto the next row is not followed there. */
export function fileLinkProvider(
  term: Terminal,
  opener: Opener,
  cwd: () => string,
): ILinkProvider {
  return {
    provideLinks(y, done) {
      const row = term.buffer.active.getLine(y - 1);
      if (!row) return done(undefined);
      // The text, and the cell each of its UTF-16 units sits in: a CJK
      // character takes two cells and an emoji two units, so they differ.
      let text = '';
      const cellAt: number[] = [];
      for (let x = 0; x < row.length; x++) {
        const cell = row.getCell(x);
        if (!cell || cell.getWidth() === 0) continue;
        const chars = cell.getChars() || ' ';
        text += chars;
        for (let i = 0; i < chars.length; i++) cellAt.push(x);
      }
      const found = findPaths(text);
      if (!found.length) return done(undefined);
      opener.resolvePaths({ paths: found.map((m) => m.path), cwd: cwd() }).then(
        (real) =>
          done(
            found.flatMap((m, i): ILink[] => {
              const path = real[i];
              if (!path) return [];
              return [
                {
                  // xterm's cells are 1-based and its range inclusive.
                  range: {
                    start: { x: cellAt[m.start] + 1, y },
                    end: { x: cellAt[m.end - 1] + 1, y },
                  },
                  text: text.slice(m.start, m.end),
                  activate(e) {
                    // A plain click belongs to selecting text.
                    if (e.metaKey) void opener.openFile({ path, line: m.line, col: m.col });
                  },
                  hover: () => void opener.hoverFile({ path }),
                  leave: () => void opener.hoverFile({ path: null }),
                },
              ];
            }),
          ),
        () => done(undefined),
      );
    },
  };
}
