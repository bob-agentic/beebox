import type { IBuffer, ILink, ILinkProvider, Terminal } from '@xterm/xterm';
import { rowText } from './file-links';
import { sentRows } from './sent-messages';

/** An `[Image #N]` in a terminal line — the tag Claude Code and Codex print
 *  where an image was pasted. `start`/`end` index the line's text, end
 *  exclusive. */
export interface ImageTag {
  start: number;
  end: number;
  n: number;
}

export function findImageTags(text: string): ImageTag[] {
  return [...text.matchAll(/\[Image #(\d+)\]/g)].map((m) => ({
    start: m.index,
    end: m.index + m[0].length,
    n: +m[1],
  }));
}

/** What opening one needs: its number, and the row it was on — Codex
 *  numbers from 1 in every message, and the row says which message. */
export type OpenImage = (n: number, row: string) => void;

/** Links over the image tags in a row. A plain click opens one: unlike a
 *  path, the tag is never text anyone wants to select. */
export function imageLinkProvider(term: Terminal, open: OpenImage, enabled: () => boolean): ILinkProvider {
  return {
    provideLinks(y, done) {
      const line = enabled() ? rowText(term, y - 1) : null;
      const tags = line ? findImageTags(line.text) : [];
      if (!line || !tags.length) return done(undefined);
      const { text, cellAt } = line;
      done(
        tags.map(
          (m): ILink => ({
            range: {
              start: { x: cellAt[m.start] + 1, y },
              end: { x: cellAt[m.end - 1] + 1, y },
            },
            text: text.slice(m.start, m.end),
            decorations: { pointerCursor: true, underline: true },
            activate: () => open(m.n, text),
          }),
        ),
      );
    },
  };
}

/** The tag under a tap, if any. xterm has no touch handling of its own, so
 *  this works from the screen's geometry: the row tapped and its neighbours,
 *  and a cell of slack either side — a fingertip covers more than a glyph. */
export function imageAtPoint(term: Terminal, x: number, y: number): { n: number; row: string } | null {
  const screen = term.element?.querySelector('.xterm-screen');
  if (!screen || !term.cols || !term.rows) return null;
  const r = screen.getBoundingClientRect();
  const cellW = r.width / term.cols;
  const cellH = r.height / term.rows;
  const col = Math.floor((x - r.left) / cellW);
  const at = (y - r.top) / cellH;
  const row = Math.floor(at);
  if (col < 0 || col >= term.cols || row < 0 || row >= term.rows) return null;
  // The row itself first, then whichever neighbour the finger was nearer.
  const near = at - row < 0.5 ? [row, row - 1, row + 1] : [row, row + 1, row - 1];
  const top = term.buffer.active.viewportY;
  for (const i of near) {
    if (i < 0 || i >= term.rows) continue;
    const line = rowText(term, top + i);
    if (!line) continue;
    for (const m of findImageTags(line.text)) {
      const from = line.cellAt[m.start] - 1;
      const to = line.cellAt[m.end - 1] + 1;
      if (col >= from && col <= to) return { n: m.n, row: line.text };
    }
  }
  return null;
}

/** The message you sent with image `#n` in it: the row it starts on, and
 *  its text without the prompt mark or the tags. Claude's numbers are the session's,
 *  so the latest message with the tag is the one; Codex starts at 1 in every
 *  message, so the latest one holding `row` — the row the tag was clicked on
 *  — is preferred. Null when the message is no longer in the terminal's
 *  history. */
export function findImageMessage(buf: IBuffer, n: number, row: string): { row: number; text: string } | null {
  const tag = `[Image #${n}]`;
  const strip = (s: string) => s.replace(/\s+/g, '');
  const hint = strip(row);
  let found: { row: number; text: string } | null = null;
  let hinted: { row: number; text: string } | null = null;
  for (const start of sentRows(buf)) {
    const lines: string[] = [];
    let y = start;
    for (; y < buf.length && y < start + 40; y++) {
      const line = buf.getLine(y);
      const text = line?.translateToString(true) ?? '';
      // It ends at a blank line, or where the agent's own `⎿` lines under
      // it begin: the image tag, hook output and the like.
      if (y > start && !line?.isWrapped && (!text.trim() || /^\s*⎿/.test(text))) break;
      // A wrapped row carries on the one before it, with no break between.
      if (line?.isWrapped && lines.length) lines[lines.length - 1] += text;
      else lines.push(text);
    }
    // The `⎿ [Image #N]` under a Claude prompt still says which images it had.
    let after = '';
    for (const end = y; y < buf.length && y < end + 4; y++) {
      const t = buf.getLine(y)?.translateToString(true) ?? '';
      if (!/^\s*⎿/.test(t)) break;
      after += t;
    }
    const whole = lines.join('\n') + after;
    if (!whole.includes(tag)) continue;
    const text = lines
      .join(' ')
      .replace(/^[❯›]\s*/, '')
      .replace(/\[Image #\d+\]/g, ' ')
      .replace(/\s+/g, ' ')
      .trim();
    found = { row: start, text };
    if (hint && strip(whole).includes(hint)) hinted = found;
  }
  return hinted ?? found;
}
