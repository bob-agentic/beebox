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
/** `path`, `path:12`, `path:12:3`, with sentence punctuation left behind —
 *  and the end of a range, `path:87-94`, since an editor opens at its start. */
const PARTS = /^((.+?)(?::(\d+)(?::(\d+))?)?)(?:(?<=:\d+)-\d+)?[.:]*$/;
/** A bare name only counts with an extension, or as a dotfile: `Pane.svelte`
 *  and `.env`, not `done`. */
const BARE = /^(?:[\w@+-][\w.@+-]*\.[A-Za-z]\w*|\.[A-Za-z][\w.-]*)$/;

/** Everything in `text` that looks like a path. Only a guess — whether it
 *  names a real file is for whoever can see the disk to decide. */
export function findPaths(text: string): PathMatch[] {
  const out: PathMatch[] = [];
  for (const m of text.matchAll(TOKEN)) {
    // `DOC=/a/b.md` and `--file=a.md`: the path is what follows the `=`.
    const lead = /^-*[A-Za-z_][\w-]*=/.exec(m[0])?.[0].length ?? 0;
    const token = m[0].slice(lead);
    const at = m.index + lead;
    if (token.includes('://')) continue;
    const p = PARTS.exec(token);
    if (!p) continue;
    const [, whole, path, line, col] = p;
    if (!/\w/.test(path) || !(path.includes('/') || BARE.test(path))) continue;
    out.push({
      start: at,
      end: at + whole.length,
      path,
      line: line ? +line : undefined,
      col: col ? +col : undefined,
    });
  }
  return out;
}

/** A buffer row's text, and the cell each of its UTF-16 units sits in: a
 *  CJK character takes two cells and an emoji two units, so they differ. */
export function rowText(term: Terminal, y: number): { text: string; cellAt: number[] } | null {
  const row = term.buffer.active.getLine(y);
  if (!row) return null;
  let text = '';
  const cellAt: number[] = [];
  for (let x = 0; x < row.length; x++) {
    const cell = row.getCell(x);
    if (!cell || cell.getWidth() === 0) continue;
    const chars = cell.getChars() || ' ';
    text += chars;
    for (let i = 0; i < chars.length; i++) cellAt.push(x);
  }
  return { text, cellAt };
}

/** The desktop shell's side: which paths exist, and opening one. */
interface Opener {
  resolvePaths(o: { paths: string[]; cwd: string }): Promise<(string | null)[]>;
  openFile(o: { path: string; line?: number; col?: number }): Promise<void>;
  /** The file under the mouse, for the right-click menu to offer. */
  hoverFile(o: { path: string | null }): Promise<void>;
}

/** How long an answer about a path is trusted before it is asked again — as
 *  VS Code's terminal does. */
export const RESOLVE_TTL_MS = 10_000;

/** Which paths exist, remembered. While an agent is thinking its TUI redraws
 *  the rows around the cursor several times a second, and xterm drops the
 *  hovered link on every redraw and asks again. Answered from here the link
 *  is back in the same frame; asked of the shell each time, it flickered.
 *  A stale answer is still given at once and refreshed behind it, so expiry
 *  never shows either. */
export class ResolveCache {
  private known = new Map<string, { real: string | null; at: number }>();
  /** Questions already on their way, so a second asker waits for the same
   *  answer instead of asking again — or giving up on the path. */
  private pending = new Map<string, Promise<void>>();

  constructor(
    private resolve: Opener['resolvePaths'],
    private now: () => number = Date.now,
  ) {}

  private static key(cwd: string, path: string) {
    return `${cwd}\0${path}`;
  }

  /** Every answer if all are known — stale ones too — or null. Stale ones
   *  are refreshed behind the answer; a failed refresh keeps the old one. */
  peek(cwd: string, paths: string[]): (string | null)[] | null {
    const hits = paths.map((p) => this.known.get(ResolveCache.key(cwd, p)));
    if (hits.some((h) => h === undefined)) return null;
    const stale = paths.filter((_, i) => this.now() - hits[i]!.at >= RESOLVE_TTL_MS);
    if (stale.length) this.ask(cwd, stale).catch(() => {});
    return hits.map((h) => h!.real);
  }

  /** Asks the shell about whatever is not known yet, then answers all. */
  async get(cwd: string, paths: string[]): Promise<(string | null)[]> {
    const missing = paths.filter((p) => !this.known.has(ResolveCache.key(cwd, p)));
    if (missing.length) await this.ask(cwd, missing);
    return paths.map((p) => this.known.get(ResolveCache.key(cwd, p))?.real ?? null);
  }

  private ask(cwd: string, paths: string[]): Promise<void> {
    const waits: Promise<void>[] = [];
    const todo = [...new Set(paths)].filter((p) => {
      const already = this.pending.get(ResolveCache.key(cwd, p));
      if (already) waits.push(already);
      return !already;
    });
    if (todo.length) {
      const asked = this.resolve({ paths: todo, cwd })
        .then((real) => {
          // Bounded: a long session prints thousands of distinct paths.
          if (this.known.size > 2000) this.known.clear();
          const at = this.now();
          todo.forEach((p, i) => this.known.set(ResolveCache.key(cwd, p), { real: real[i] ?? null, at }));
        })
        .finally(() => todo.forEach((p) => this.pending.delete(ResolveCache.key(cwd, p))));
      todo.forEach((p) => this.pending.set(ResolveCache.key(cwd, p), asked));
      waits.push(asked);
    }
    return Promise.all(waits).then(() => {});
  }
}

/** Links over the file paths in a row, for `registerLinkProvider`. A path
 *  wrapped onto the next row is not followed there. */
export function fileLinkProvider(
  term: Terminal,
  opener: Opener,
  cwd: () => string,
): ILinkProvider {
  const cache = new ResolveCache((o) => opener.resolvePaths(o));
  // The file under the mouse, as last told to the shell. A redraw drops the
  // link and hands it straight back; telling the shell "none" and then the
  // same file again each time would be noise, so a leave waits a tick to see
  // whether the same link comes back.
  let hovered: string | null = null;
  let leaving: ReturnType<typeof setTimeout> | null = null;
  const hover = (path: string) => {
    if (leaving) clearTimeout(leaving);
    leaving = null;
    if (hovered !== path) void opener.hoverFile({ path: (hovered = path) });
  };
  const leave = () => {
    if (leaving) clearTimeout(leaving);
    leaving = setTimeout(() => {
      leaving = null;
      if (hovered !== null) void opener.hoverFile({ path: (hovered = null) });
    }, 0);
  };

  return {
    provideLinks(y, done) {
      const line = rowText(term, y - 1);
      if (!line) return done(undefined);
      const { text, cellAt } = line;
      const found = findPaths(text);
      if (!found.length) return done(undefined);
      const links = (real: (string | null)[]) =>
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
              hover: () => hover(path),
              leave,
            },
          ];
        });
      const dir = cwd();
      const paths = found.map((m) => m.path);
      // Synchronous when known: that is what keeps a redraw from showing.
      const known = cache.peek(dir, paths);
      if (known) return done(links(known));
      cache.get(dir, paths).then(
        (real) => done(links(real)),
        () => done(undefined),
      );
    },
  };
}
