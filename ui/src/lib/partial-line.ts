/** Keeps zsh's partial-line marker working at any width.
 *
 *  After every command zsh prints a reverse-video `%`, then spaces — one fewer
 *  than its terminal's width — then a carriage return. Where the command's
 *  output ended with a newline, the cursor started at the line's start: the
 *  spaces fill the line exactly, the return comes back over the `%`, and the
 *  prompt covers it. Where the output did not — `cat` of a file with no final
 *  newline — the spaces run past the end and wrap, so the `%` stays at the end
 *  of the last line and the prompt starts on the next. That is zsh saying the
 *  output stopped mid-line, and every terminal shows it so.
 *
 *  Both depend on the count matching the width, fixed when the bytes were
 *  written. Drawn at another width — the phone, or history from before a
 *  resize — the spaces wrap where they should not and leave a stray `%` at the
 *  top of the screen, or fail to wrap and let the prompt overwrite the output's
 *  last line. And even at the right width, the spaces after a mid-line `%` are
 *  real cells: narrow the terminal later and they reflow into blank lines.
 *
 *  So the marker becomes what it was for. After text on the line: the `%` and
 *  a new line, no spaces, at any width. At a line's start: left alone where
 *  the width matches — the prompt erases it — and dropped where it does not.
 *  Where it cannot be told which (the cursor was last moved by an escape
 *  sequence), it is left alone at the matching width and dropped otherwise.
 *
 *  The width in effect is `cols` until one of the daemon's width marks
 *  (`ESC ] 7788 ; cols BEL`, see pane-term.ts) changes it — the same marks
 *  that resize the terminal as the bytes are drawn. Output arrives in chunks,
 *  and the text before a marker may have come in an earlier one, so `line`
 *  carries from one chunk to the next whether its line has text on it: true,
 *  false, or null for unknown. Matched tightly: a `%` in real output is never
 *  touched.
 */
export function fixPartialLineMarkers(data: Uint8Array, cols: number, line: LineState): Uint8Array {
  const text = new TextDecoder().decode(data);
  let fixed = text;
  if (text.includes('\u001b[7m%')) {
    let width = cols;
    fixed = text.replace(STEP, (all, mark: string | undefined, pad: string | undefined, at: number) => {
      if (mark !== undefined) {
        width = +mark;
        return all;
      }
      // Padding running to the end may go on in the next chunk: its count is
      // unknown, so it is left as it came.
      if (at + all.length === text.length) return all;
      const before = textBefore(text, at, line.text);
      if (before === true) return '\u001b[7m%\u001b[27m\r\n';
      return pad!.length + 1 === width ? all : '';
    });
  }
  line.text = textBefore(fixed, fixed.length, line.text);
  // A character cut in two by the chunk's edge decodes to U+FFFD, and would
  // be encoded back as that: better to leave this chunk as it came.
  if (fixed === text || text.startsWith('\ufffd') || text.endsWith('\ufffd')) return data;
  return new TextEncoder().encode(fixed);
}

/** Whether the line the cursor is on has text on it, as of the last chunk. */
export interface LineState {
  text: boolean | null;
}

// A width mark, or the marker: bold, reverse video, `%`, the attribute resets
// zsh follows it with, then the padding.
// eslint-disable-next-line no-control-regex
const STEP = /\u001b\]7788;(\d+)\u0007|(?:\u001b\[1m)?\u001b\[7m%(?:\u001b\[[0-9;]*m)*( +)/g;
// Escape sequences that put the cursor somewhere this cannot follow: cursor
// movement and positioning, restore, index and reverse index.
// eslint-disable-next-line no-control-regex
const MOVES = /\u001b\[[0-?]*[A-Hdf]|\u001b[78DEM]/;
// Everything that prints nothing: CSI (any parameters, `ESC[>4;1m` included),
// OSC, charset designations, two-byte escapes, then the C0 controls left.
// eslint-disable-next-line no-control-regex
const SILENT = /\u001b\[[0-?]*[ -/]*[@-~]|\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)|\u001b[()*+][ -~]|\u001b[ -~]|[\u0000-\u001f\u007f]/g;

/** Whether the line holds text just before `at`: from what is on it since the
 *  last line break, or — with no break in this chunk — from `carried` too. */
function textBefore(text: string, at: number, carried: boolean | null): boolean | null {
  const brk = Math.max(text.lastIndexOf('\n', at - 1), text.lastIndexOf('\r', at - 1));
  const tail = text.slice(brk + 1, at);
  if (MOVES.test(tail)) return null;
  if (tail.replace(SILENT, '').length > 0) return true;
  return brk >= 0 ? false : carried;
}
